
// SPDX-License-Identifier: AGPL-3.0-only
// one-off (to be reverted): Workflow #335 S5b test entries, appended to host.rs by the one-off
// workflow on the PR head only.
#[cfg(test)]
impl RuntimeHost {
    /// A manual contained run through the host's own entry: the success state, or the failure's
    /// code and native detail (which the IPC receipt does not carry).
    pub(crate) fn one_off_335s5b_run_contained_task(
        &self,
        request: &RuntimeRequest,
    ) -> Result<String, (String, Option<String>)> {
        let shared = self.shared.as_ref().expect("test Runtime host is open");
        let validated = request.validate().expect("test Runtime request is valid");
        let RuntimeOperation::RunContainedTask {
            instance_alias,
            holder_id,
            request: task_request,
        } = request.operation()
        else {
            panic!("the one-off run expects a contained task request");
        };
        let connection_id = ConnectionId::new(9_335).expect("one-off connection");
        match shared.run_contained_task(
            request,
            &validated,
            instance_alias,
            *holder_id,
            task_request,
            connection_id,
        ) {
            Ok(success) => Ok(format!("{:?}", success.state)),
            Err(failure) => Err((
                failure.error.code().to_owned(),
                failure
                    .error
                    .diagnostics()
                    .native_detail()
                    .map(|detail| detail.text().to_owned()),
            )),
        }
    }

    /// Runs `request` as the instance's startup package on this thread.
    pub(crate) fn one_off_335s5b_run_startup_package(
        &self,
        instance_alias: &str,
        request: ContainedTaskRequest,
    ) -> RuntimeHostResult<Result<String, &'static str>> {
        let shared = self.shared.as_ref().expect("test Runtime host is open");
        let resolved = shared
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?;
        let issuer = shared.events.issuer();
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let control_request_id = *issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let pending = startup_package::PendingStartupPackage {
            instance_id: resolved.instance_id(),
            instance_alias: instance_alias.to_owned(),
            request,
            causation_id,
            control_request_id,
            run: startup_package::HostPackageRun::StartupPackage,
        };
        Ok(shared
            .run_pending_startup_package(&pending)?
            .map(|success| format!("{:?}", success.state)))
    }
}
