// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 AO-4q, test row AO4-c (model-ledger-observer v1.4.1 §14.2; register W1;
// ledger-observer.md §13): a status read (Status, MonitorStatus, ProjectInterface and
// DiscoverInstances) reads memory only and appends no ledger record, and its answer's source
// names the ledger's committed head. The two stores' (lineage, revision) arrive with AO-3.
//
// The ledger head is read through an event query, a read that appends nothing. No lease is
// held and no performance thread runs, so nothing else writes between two reads.

use super::*;

// The ledger's committed head: the snapshot position of a first event page.
fn ledger_head(reader: &mut TestClient) -> u64 {
    let request = reader.request(RuntimeOperation::QueryEvents {
        query: EventQuery::default(),
        profile: ProjectionProfile::Concise,
        page: RuntimeEventQueryPageRequest::new(1, None).expect("event page"),
    });
    let receipt = reader.send(&request);
    let Some(RuntimeResult::EventPage { page }) = receipt.result() else {
        panic!("expected an event page, got {:?}", receipt.result());
    };
    page.snapshot_ledger_position()
}

#[test]
fn ao4_c_status_reads_append_no_record_and_name_the_committed_head() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one("node.a", instance_id(), Arc::new(FakeState::default()))
                .with_discovery(ProviderInstanceDiscovery {
                    provider_version: "fake.provider.1".to_owned(),
                    instances: vec![ProviderDiscoveredInstance {
                        instance_index: 0,
                        instance_name: "neutral.instance".to_owned(),
                        adb_host: None,
                        adb_port: None,
                        running: false,
                        android_version: None,
                    }],
                }),
        ),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);

    for operation in [
        RuntimeOperation::Status,
        RuntimeOperation::MonitorStatus,
        RuntimeOperation::ProjectInterface {
            request: ProjectInterfaceRequest::current(),
        },
        RuntimeOperation::DiscoverInstances,
    ] {
        let head = ledger_head(&mut reader);
        let request = client.request(operation);
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{:?}",
            request.operation()
        );
        let source = match receipt.result() {
            Some(RuntimeResult::Status { status }) => status.source().cloned(),
            Some(RuntimeResult::MonitorStatus { status }) => status.source().cloned(),
            Some(RuntimeResult::ProjectInterface { response }) => response
                .current()
                .and_then(|current| current.source.clone()),
            Some(RuntimeResult::InstancesDiscovered { discovery }) => {
                Some(discovery.source().clone())
            }
            other => panic!("unexpected result {other:?}"),
        }
        .expect("the answer's state source");

        assert_eq!(
            source.ledger_committed_head,
            head,
            "{:?} names the committed head",
            request.operation()
        );
        assert_eq!(
            ledger_head(&mut reader),
            head,
            "{:?} appends no ledger record",
            request.operation()
        );
        assert!(
            projected_events(
                &mut reader,
                EventQuery {
                    request_id: Some(request.request_id()),
                    ..EventQuery::default()
                },
            )
            .is_empty(),
            "{:?} leaves no record of its request",
            request.operation()
        );
    }

    drop(reader);
    drop(client);
    host.close().expect("close host");
}
