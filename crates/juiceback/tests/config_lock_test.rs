mod common;

use juiceback::error::AppError;

#[test]
fn poisoned_config_lock_returns_error_instead_of_panicking() {
    let state = common::test_state();
    let lock = std::sync::Arc::clone(&state.jh_config);
    let _ = std::thread::spawn(move || {
        let _guard = lock.write().unwrap();
        panic!("intentional poison for test");
    })
    .join();
    assert!(state.jh_config.is_poisoned());
    let err = state.juicehost_config().unwrap_err();
    assert!(
        matches!(err, AppError::Internal(_)),
        "expected Internal, got {err:?}"
    );
}
