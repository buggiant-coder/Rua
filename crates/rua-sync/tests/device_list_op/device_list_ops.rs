use rua_sync::device_list::{DeviceEntry, DeviceList};

fn make_entry(id: u8) -> DeviceEntry {
    DeviceEntry {
        device_id: [id; 8],
        public_key: [id; 32],
        label: String::new(),
        friendly_name: String::new(),
        added_at_ms: 0,
        last_seen_ms: 0,
        direct_addrs: vec![],
    }
}

#[test]
fn add_device_deduplicates() {
    let mut list = DeviceList::new();
    let entry = make_entry(1);
    list.add_device(entry.clone());
    list.add_device(entry.clone());
    assert_eq!(list.devices.len(), 1);
}

#[test]
fn contains_device_works() {
    let mut list = DeviceList::new();
    let entry = make_entry(5);
    assert!(!list.contains_device(&[5; 8]));
    list.add_device(entry);
    assert!(list.contains_device(&[5; 8]));
}

#[test]
fn public_keys_returns_all() {
    let mut list = DeviceList::new();
    list.add_device(make_entry(1));
    list.add_device(make_entry(2));
    let keys = list.public_keys();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], [1u8; 32]);
    assert_eq!(keys[1], [2u8; 32]);
}
