use super::*;
use std::cell::{Cell, RefCell};
struct Fake {
    permission: RefCell<String>,
    requested: Cell<bool>,
    read: Cell<bool>,
    fail: Cell<bool>,
}
impl Fake {
    fn new() -> Self {
        Self {
            permission: RefCell::new("notDetermined".into()),
            requested: Cell::new(false),
            read: Cell::new(false),
            fail: Cell::new(false),
        }
    }
}
impl AddressBook for Fake {
    fn supported(&self) -> bool {
        true
    }
    fn permission(&self) -> String {
        self.permission.borrow().clone()
    }
    fn request(&self) -> Result<(), String> {
        self.requested.set(true);
        *self.permission.borrow_mut() = "authorized".into();
        Ok(())
    }
    fn sources(&self) -> Result<Vec<ContactSource>, String> {
        Ok(vec![ContactSource {
            id: "icloud".into(),
            name: "iCloud".into(),
        }])
    }
    fn contacts(&self, selected: &[String]) -> Result<Vec<SystemContact>, String> {
        self.read.set(true);
        if self.fail.get() {
            return Err("temporary failure".into());
        }
        assert_eq!(selected, &["icloud"]);
        Ok(vec![SystemContact {
            id: "native".into(),
            source_id: "icloud".into(),
            display_name: Some("Ada".into()),
            addresses: vec!["ada@example.com".into()],
            ..Default::default()
        }])
    }
}
#[test]
fn access_is_opt_in_and_discovery_does_not_import_contacts() {
    let db = Database::open_memory();
    let fake = Fake::new();
    refresh_with(&db, &fake, None).unwrap();
    assert!(!fake.requested.get());
    assert!(!fake.read.get());
    let status = discover_with(&db, &fake).unwrap();
    assert!(fake.requested.get());
    assert!(status.connection.selected_sources.is_empty());
    assert!(!fake.read.get());
    let status = refresh_with(&db, &fake, Some(vec!["icloud".into()])).unwrap();
    assert_eq!(status.contact_count, 1);
    fake.fail.set(true);
    assert!(refresh_with(&db, &fake, None).is_err());
    assert_eq!(db.system_contact_count().unwrap(), 1);
    *fake.permission.borrow_mut() = "denied".into();
    assert_eq!(status_with(&db, &fake).unwrap().contact_count, 0);
    assert!(db.list_saved_contact_profiles().unwrap().is_empty());
}
#[test]
fn disconnect_works_even_after_permission_is_revoked() {
    let db = Database::open_memory();
    let fake = Fake::new();
    discover_with(&db, &fake).unwrap();
    refresh_with(&db, &fake, Some(vec!["icloud".into()])).unwrap();
    *fake.permission.borrow_mut() = "denied".into();
    let status = refresh_with(&db, &fake, Some(vec![])).unwrap();
    assert!(status.connection.selected_sources.is_empty());
    assert!(status.connection.sources.is_empty());
    assert_eq!(status.contact_count, 0);
}
