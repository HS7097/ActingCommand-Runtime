// SPDX-License-Identifier: AGPL-3.0-only

use crate::{FatalState, InputBackendProvider, RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{InputAction, RuntimeErrorCode};
use actingcommand_device::{DeviceResult, InputBackend};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

enum BackendCommand {
    Input {
        action: InputAction,
        response: SyncSender<DeviceResult<()>>,
    },
    Close {
        response: SyncSender<DeviceResult<()>>,
    },
}

pub(crate) struct BackendWorker {
    sender: Option<SyncSender<BackendCommand>>,
    join: Option<JoinHandle<RuntimeHostResult<()>>>,
    fatal: FatalState,
}

impl BackendWorker {
    pub(crate) fn open(
        provider: Arc<dyn InputBackendProvider>,
        instance_alias: String,
        fatal: FatalState,
    ) -> RuntimeHostResult<Self> {
        let (sender, receiver) = mpsc::sync_channel(8);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker_fatal = fatal.clone();
        let join = thread::Builder::new()
            .name("actingcommand-input-backend".to_string())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    run_backend_worker(provider, instance_alias, receiver, ready_sender)
                }));
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        let error = RuntimeHostError::fatal(
                            "backend_worker_panicked",
                            "run_input_backend",
                            RuntimeErrorCode::RuntimeFatal,
                        );
                        worker_fatal.mark(error.clone())?;
                        Err(error)
                    }
                }
            })
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "backend_worker_spawn_failed",
                    "open_input_backend",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                sender: Some(sender),
                join: Some(join),
                fatal,
            }),
            Ok(Err(error)) => {
                drop(sender);
                join_worker(join)?;
                Err(RuntimeHostError::backend_open(&error))
            }
            Err(_) => {
                drop(sender);
                let join_result = join_worker(join);
                let error = RuntimeHostError::fatal(
                    "backend_worker_unavailable",
                    "open_input_backend",
                    RuntimeErrorCode::RuntimeFatal,
                );
                fatal.mark(error.clone())?;
                join_result?;
                Err(error)
            }
        }
    }

    pub(crate) fn execute(&self, action: InputAction) -> RuntimeHostResult<()> {
        let sender = self.sender.as_ref().ok_or_else(|| {
            RuntimeHostError::fatal(
                "backend_worker_closed",
                "execute_input_backend",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let (response, receiver) = mpsc::sync_channel(1);
        sender
            .send(BackendCommand::Input { action, response })
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "backend_worker_unavailable",
                    "execute_input_backend",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        receiver
            .recv()
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "backend_worker_unavailable",
                    "execute_input_backend",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?
            .map_err(|error| RuntimeHostError::backend_operation(&error))
    }

    pub(crate) fn close(&mut self) -> RuntimeHostResult<()> {
        let Some(sender) = self.sender.take() else {
            return self.join_worker();
        };
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = sender.send(BackendCommand::Close { response });
        drop(sender);
        let close_result = send_result
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "backend_worker_unavailable",
                    "close_input_backend",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })
            .and_then(|()| {
                receiver
                    .recv()
                    .map_err(|_| {
                        RuntimeHostError::fatal(
                            "backend_worker_unavailable",
                            "close_input_backend",
                            RuntimeErrorCode::RuntimeFatal,
                        )
                    })?
                    .map_err(|_| RuntimeHostError::backend_close())
            });
        let join_result = self.join_worker();
        close_result.and(join_result)
    }

    fn join_worker(&mut self) -> RuntimeHostResult<()> {
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        join_worker(join)
    }
}

impl Drop for BackendWorker {
    fn drop(&mut self) {
        if self.sender.is_none() && self.join.is_none() {
            return;
        }
        if let Err(error) = self.close()
            && self.fatal.mark(error.clone()).is_err()
            && !thread::panicking()
        {
            panic!("{error}");
        }
    }
}

fn run_backend_worker(
    provider: Arc<dyn InputBackendProvider>,
    instance_alias: String,
    receiver: Receiver<BackendCommand>,
    ready: SyncSender<DeviceResult<()>>,
) -> RuntimeHostResult<()> {
    let mut backend = match provider.open(&instance_alias) {
        Ok(backend) => {
            if ready.send(Ok(())).is_err() {
                return close_backend(backend);
            }
            backend
        }
        Err(error) => {
            let runtime_error = RuntimeHostError::backend_open(&error);
            return if ready.send(Err(error)).is_ok() {
                Ok(())
            } else {
                Err(runtime_error)
            };
        }
    };
    while let Ok(command) = receiver.recv() {
        match command {
            BackendCommand::Input { action, response } => {
                let result = execute_action(backend.as_mut(), &action);
                if response.send(result).is_err() {
                    return close_backend(backend);
                }
            }
            BackendCommand::Close { response } => {
                let result = backend.close();
                let failed = result.as_ref().err().cloned();
                if response.send(result).is_err() {
                    return Err(failed.map_or_else(
                        || {
                            RuntimeHostError::fatal(
                                "backend_worker_response_lost",
                                "close_input_backend",
                                RuntimeErrorCode::RuntimeFatal,
                            )
                        },
                        |_| RuntimeHostError::backend_close(),
                    ));
                }
                return failed.map_or(Ok(()), |_| Err(RuntimeHostError::backend_close()));
            }
        }
    }
    close_backend(backend)
}

fn execute_action(backend: &mut dyn InputBackend, action: &InputAction) -> DeviceResult<()> {
    match action {
        InputAction::Tap { x, y } => backend.tap(*x, *y),
        InputAction::LongTap { x, y, duration_ms } => backend.long_tap(*x, *y, *duration_ms),
        InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        } => backend.swipe(*x1, *y1, *x2, *y2, *duration_ms),
        InputAction::Key { key } => backend.key(key),
        InputAction::Text { text } => backend.text(text),
        InputAction::Reset => backend.reset(),
    }
}

fn close_backend(mut backend: Box<dyn InputBackend>) -> RuntimeHostResult<()> {
    backend
        .close()
        .map_err(|_| RuntimeHostError::backend_close())
}

fn join_worker(join: JoinHandle<RuntimeHostResult<()>>) -> RuntimeHostResult<()> {
    join.join().map_err(|_| {
        RuntimeHostError::fatal(
            "backend_worker_panicked",
            "join_input_backend",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?
}
