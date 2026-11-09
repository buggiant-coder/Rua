use rua_server::endpoints::pairing::{PairingState, PairingStatusKind};
use rua_sync::pairing::PairingPin;

#[test]
fn consumed_pin_reports_finalizing_status_until_pairing_result_is_recorded() {
    let mut pin = PairingPin::generate();
    pin.consume().expect("consume pin");
    let state = PairingState {
        pin: Some(pin),
        initiator_running: true,
        result: None,
    };

    assert_eq!(state.status(), PairingStatusKind::Consumed);
}
