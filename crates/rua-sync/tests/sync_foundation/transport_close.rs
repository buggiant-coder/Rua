use iroh_docs::net::ConnectError;
use rua_sync::transport::is_post_reconcile_close_error;

#[test]
fn close_error_is_classified_as_post_reconcile() {
    let error = ConnectError::Close {
        error: anyhow::anyhow!("stream already stopped"),
    };

    assert!(is_post_reconcile_close_error(&error));
}

#[test]
fn connect_error_is_not_classified_as_post_reconcile() {
    let error = ConnectError::Connect {
        error: anyhow::anyhow!("no route to peer"),
    };

    assert!(!is_post_reconcile_close_error(&error));
}
