// SPDX-License-Identifier: AGPL-3.0-only

//! #71 FLK-07, slice HARNESS-DIAG (host key 168): when the test client cannot read a receipt,
//! its panic names the fatal condition the Runtime under test latched and the Runtime's
//! shutdown flag.

use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn harness_diag_t1_an_unreadable_receipt_names_the_latched_fatal_and_the_shutdown_flag() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    // A file in the artifact directory's place: the first observe cannot store its frame, the
    // Runtime latches `artifact_directory_failed` and ends the connection.
    fs::write(root.path().join("artifacts"), b"blocks artifact directory")
        .expect("block artifact directory");
    let mut client = TestClient::connect(&host);
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: "node.a".to_owned(),
    });
    let failed = client.send(&observe);
    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    assert!(
        failed
            .error_projection()
            .expect("fatal artifact error")
            .fatal
    );

    let next = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: "node.a".to_owned(),
    });
    let panic = catch_unwind(AssertUnwindSafe(|| client.send(&next)))
        .expect_err("no receipt after the Runtime latched a fatal");
    let text = panic
        .downcast_ref::<String>()
        .expect("the harness panic text");
    assert!(
        text.contains("latched_fatal=artifact_directory_failed "),
        "the panic text does not name the latched fatal: {text}"
    );
    assert!(
        text.contains("shutdown_requested=true"),
        "the panic text does not name the shutdown flag: {text}"
    );
    drop(client);
    host.close()
        .expect_err("the host closes with its latched fatal");
}
