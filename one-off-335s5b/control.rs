// SPDX-License-Identifier: AGPL-3.0-only

//! one-off (to be reverted): Workflow #335 S5b H7 on the merge-base 762dd274. The H1 state root
//! written by the PR head (`ONE_OFF_335S5B_STATE`) is opened by this build: the host starts on a
//! copy and the instance fact projection is printed as `H7|record ...` lines, compared with the
//! head's lines in the job.

use super::one_off_335s5b_common::*;
use super::*;

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create copy directory");
    for entry in fs::read_dir(from).expect("read directory") {
        let entry = entry.expect("directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

#[test]
fn one_off_335s5b_control_h7_opens_the_head_state() {
    let Ok(source) = std::env::var("ONE_OFF_335S5B_STATE") else {
        out("H7", "no state root given");
        return;
    };
    let source = PathBuf::from(source);
    let root = TempDir::new().expect("tempdir");
    copy_tree(&source.join("state"), root.path());
    let instance: InstanceId = serde_json::from_slice(
        &fs::read(source.join("instance_id.json")).expect("instance id"),
    )
    .expect("instance id JSON");
    let clock_at: u64 = fs::read_to_string(source.join("clock_unix_ms.txt"))
        .expect("clock")
        .trim()
        .parse()
        .expect("clock value");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(clock_at + 1_000, clock_at + 1_000));
    let host = match RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(POLICY_INSTANCE_ALIAS, instance, state)),
    ) {
        Ok(host) => host,
        Err(error) => {
            out("H7", format!("control start failed code={}", error.code()));
            return;
        }
    };
    out("H7", "control build started on the head state root");
    let snapshot = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("instance facts");
    for record in &snapshot.records {
        out(
            "H7",
            format!(
                "record {}",
                serde_json::to_string(record).unwrap_or_default()
            ),
        );
    }
    out(
        "H7",
        format!(
            "control fatal={:?}",
            host.fatal_error().expect("health").map(|error| error.code())
        ),
    );
    host.close().expect("close host");
}
