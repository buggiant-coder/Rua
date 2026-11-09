use rua_server::services::device_network_availability::{
    DeviceNetworkAvailability, DeviceNetworkAvailabilityState,
};

const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[test]
fn availability_transitions_are_ordered_and_duplicate_states_are_collapsed() {
    let availability = DeviceNetworkAvailability::new();
    let initial_revision = availability.snapshot().revision;
    assert!(initial_revision > 0);
    assert!(initial_revision <= JS_MAX_SAFE_INTEGER);
    assert_eq!(
        availability.snapshot().state,
        DeviceNetworkAvailabilityState::Suspended
    );

    availability.mark_starting();
    availability.mark_starting();
    let starting_revision = availability.snapshot().revision;
    assert!(starting_revision > initial_revision);

    availability.mark_failed("bind failed");
    let failed_revision = availability.snapshot().revision;
    assert!(failed_revision > starting_revision);
    assert_eq!(
        availability.snapshot().state,
        DeviceNetworkAvailabilityState::Failed
    );

    availability.mark_starting();
    availability.mark_ready();
    availability.mark_suspended();
    assert!(availability.snapshot().revision > failed_revision);
    assert!(availability.snapshot().revision <= JS_MAX_SAFE_INTEGER);
    assert_eq!(
        availability.snapshot().state,
        DeviceNetworkAvailabilityState::Suspended
    );
}
