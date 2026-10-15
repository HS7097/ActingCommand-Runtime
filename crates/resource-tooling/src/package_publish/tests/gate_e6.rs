// SPDX-License-Identifier: AGPL-3.0-only

// E-6 (test plan v2.2 section 3.10 E-6 and section 4 item 10; #332 G-a, PR #552; exec key 31):
// two reclaimers of one stale publication lock in the two orders that
// `concurrent_stale_reclaim_has_one_winner` reaches only by chance. The reclaim checkpoints of
// `PublicationEnvironment` hold one reclaimer between its check and its action while the other
// goes ahead; both reclaimers are threads of one process, as in that test.
// - E6-T1: A judged the lock stale and stops; B reclaims it and acquires; A goes on, finds the
//   lock changed, retries and is refused by B, a live owner. B is the one winner, its lock records
//   are unchanged, and no tombstone or claim file is left.
// - E6-T2: A holds its claim and stops; B finds the stale lock and the claim held by a live
//   owner, and retries; A goes on, moves the stale lock away and acquires; B is refused by A, a
//   live owner. A is the one winner, and the claim is released.
// Every wait returns as soon as its message arrives; `GATE_WAIT` only bounds a broken run.

use super::*;

const GATE_WAIT: Duration = Duration::from_secs(120);
// RejectLiveOwner's refusal (`PublicationLock::acquire_with_policy`).
const LIVE_OWNER_REFUSAL: &str = "publication output is locked by a live owner";

// A reclaimer thread named `name`: it begins a publication of `output`, reports whether it won
// (or the refusal's message) and holds a won transaction until the test ends it.
struct Reclaimer {
    result: mpsc::Receiver<Result<(), String>>,
    finish: mpsc::Sender<()>,
    thread: thread::JoinHandle<()>,
}

fn reclaimer(name: &str, env: &FakeEnvironment, output: &Path) -> Reclaimer {
    let env = env.clone();
    let output = output.to_path_buf();
    let (result_sender, result) = mpsc::channel();
    let (finish, finish_receiver) = mpsc::channel::<()>();
    let thread = thread::Builder::new()
        .name(name.to_string())
        .spawn(
            move || match PackagePublicationTransaction::begin_single_with(&output, &env) {
                Ok(transaction) => {
                    result_sender.send(Ok(())).expect("report the win");
                    finish_receiver
                        .recv_timeout(GATE_WAIT)
                        .expect("the test ends the winner");
                    transaction.abort().unwrap();
                }
                Err(error) => result_sender
                    .send(Err(error.message))
                    .expect("report the refusal"),
            },
        )
        .expect("spawn the reclaimer");
    Reclaimer {
        result,
        finish,
        thread,
    }
}

// An abandoned publication of `output` by process `dead_pid`, which has died; the reclaimers
// run as process `live_pid`. Returns the lock directory.
fn abandoned_lock(env: &FakeEnvironment, output: &Path, dead_pid: u32, live_pid: u32) -> PathBuf {
    let abandoned = PackagePublicationTransaction::begin_single_with(output, env).unwrap();
    drop(abandoned);
    env.set_status(dead_pid, Ok(ProcessStatus::Dead));
    env.set_current(live_pid, "new-start");
    output
        .parent()
        .unwrap()
        .join(STATE_DIRECTORY)
        .join(LOCKS_DIRECTORY)
}

// Every lock record in `locks`, by file name.
fn lock_records(locks: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(locks)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.ends_with(".lock")
                .then(|| (name, fs::read(entry.path()).unwrap()))
        })
        .collect()
}

// Claim files (`<digest>.claim`) and tombstones (`<digest>.reclaim-<owner>`) left in `locks`.
fn reclaim_residue(locks: &Path) -> Vec<String> {
    fs::read_dir(locks)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".claim") || name.contains(".reclaim-"))
        .collect()
}

// E6-T1: overtaken after the stale check.
#[test]
fn gate_e6_t1_a_reclaimer_overtaken_after_its_stale_check_is_refused_by_the_live_winner() {
    let temp = TempDir::new().unwrap();
    let env = FakeEnvironment::new(121, "old-start");
    let output = temp.path().join("package.zip");
    let locks = abandoned_lock(&env, &output, 121, 122);
    let (a_stopped, a_resume) =
        env.gate_reclaim("reclaimer-a", ReclaimCheckpoint::AfterStaleCheck, 1);

    let a = reclaimer("reclaimer-a", &env, &output);
    a_stopped
        .recv_timeout(GATE_WAIT)
        .expect("A judges the lock stale");
    let b = reclaimer("reclaimer-b", &env, &output);
    assert_eq!(
        b.result.recv_timeout(GATE_WAIT).expect("B's acquisition"),
        Ok(()),
        "B reclaims the stale lock and acquires"
    );
    let won = lock_records(&locks);
    a_resume.send(()).expect("let A go on");
    let refused = a
        .result
        .recv_timeout(GATE_WAIT)
        .expect("A's acquisition")
        .expect_err("A is refused");

    assert!(refused.contains(LIVE_OWNER_REFUSAL), "{refused}");
    assert_eq!(lock_records(&locks), won, "B's lock records changed");
    assert_eq!(reclaim_residue(&locks), Vec::<String>::new());
    b.finish.send(()).expect("end B's publication");
    b.thread.join().unwrap();
    a.thread.join().unwrap();
}

// E6-T2: the claim is held.
#[test]
fn gate_e6_t2_a_reclaimer_meeting_a_held_claim_retries_and_is_refused_by_the_claim_holder() {
    let temp = TempDir::new().unwrap();
    let env = FakeEnvironment::new(131, "old-start");
    let output = temp.path().join("package.zip");
    let locks = abandoned_lock(&env, &output, 131, 132);
    let (a_stopped, a_resume) = env.gate_reclaim("reclaimer-a", ReclaimCheckpoint::ClaimHeld, 1);
    // B's second stale check comes after its first reclaim met A's claim and was told to retry.
    let (b_stopped, b_resume) =
        env.gate_reclaim("reclaimer-b", ReclaimCheckpoint::AfterStaleCheck, 2);

    let a = reclaimer("reclaimer-a", &env, &output);
    a_stopped
        .recv_timeout(GATE_WAIT)
        .expect("A holds its claim");
    let b = reclaimer("reclaimer-b", &env, &output);
    b_stopped
        .recv_timeout(GATE_WAIT)
        .expect("B meets the held claim and retries");
    a_resume.send(()).expect("let A go on");
    assert_eq!(
        a.result.recv_timeout(GATE_WAIT).expect("A's acquisition"),
        Ok(()),
        "A moves the stale lock away and acquires"
    );
    let won = lock_records(&locks);
    b_resume.send(()).expect("let B go on");
    let refused = b
        .result
        .recv_timeout(GATE_WAIT)
        .expect("B's acquisition")
        .expect_err("B is refused");

    assert!(refused.contains(LIVE_OWNER_REFUSAL), "{refused}");
    assert_eq!(lock_records(&locks), won, "A's lock records changed");
    assert_eq!(reclaim_residue(&locks), Vec::<String>::new());
    a.finish.send(()).expect("end A's publication");
    a.thread.join().unwrap();
    b.thread.join().unwrap();
}
