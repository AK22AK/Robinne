use robinne::models::Record;
use robinne::store::{create_store, StoreEvent};
use tempfile::TempDir;
use tokio::time::{timeout, Duration};

#[tokio::test(flavor = "current_thread")]
async fn create_record_notifies_data_changed_subscribers() {
    let local = tokio::task::LocalSet::new();

    local
        .run_until(async {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("store-events.db");
            let (store, mut runtime) = create_store();
            let mut changes = store.subscribe_data_changes();

            let runtime_handle = tokio::task::spawn_local(async move {
                runtime.run(db_path).await;
            });

            store
                .create_record(Record::new_note("实时刷新测试".to_string()))
                .await
                .unwrap();

            let event = timeout(Duration::from_secs(1), changes.recv())
                .await
                .expect("expected store event")
                .expect("event channel should stay open");

            assert_eq!(event, StoreEvent::DataChanged);

            drop(store);
            runtime_handle.abort();
        })
        .await;
}
