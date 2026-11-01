//! Only requested keys are read, with unification disabled to keep excluded
//! accounts out of a selected container's results. No notes entitlement needed.
use super::*;
use block2::RcBlock;
use objc2::{
    rc::autoreleasepool,
    runtime::{Bool, ProtocolObject},
    AllocAnyThread,
};
use objc2_contacts::*;
use objc2_foundation::{NSArray, NSBundle, NSError, NSString};
use std::cell::RefCell;

impl AddressBook for Platform {
    fn supported(&self) -> bool {
        true
    }
    fn permission(&self) -> String {
        let status =
            unsafe { CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts) };
        match status {
            CNAuthorizationStatus::Authorized => "authorized",
            CNAuthorizationStatus::Limited => "limited",
            CNAuthorizationStatus::Denied => "denied",
            CNAuthorizationStatus::Restricted => "restricted",
            _ => "notDetermined",
        }
        .into()
    }
    fn request(&self) -> Result<(), String> {
        if self.permission() != "notDetermined" {
            return Ok(());
        }
        autoreleasepool(|_| unsafe {
            // An unbundled development executable has no usage description;
            // asking TCC from it would terminate the process.
            if NSBundle::mainBundle()
                .objectForInfoDictionaryKey(&NSString::from_str("NSContactsUsageDescription"))
                .is_none()
            {
                return Err("Connect Mac Contacts from an installed build of ThreeStrands.".into());
            }
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let callback = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
                let _ = sender.send(granted.as_bool());
            });
            CNContactStore::new()
                .requestAccessForEntityType_completionHandler(CNEntityType::Contacts, &callback);
            receiver
                .recv()
                .map_err(|_| "Contacts permission request was interrupted".to_string())?;
            Ok(())
        })
    }
    fn sources(&self) -> Result<Vec<ContactSource>, String> {
        autoreleasepool(|_| unsafe {
            let store = CNContactStore::new();
            let sources = store
                .containersMatchingPredicate_error(None)
                .map_err(|e| e.to_string())?;
            Ok(sources
                .iter()
                .map(|source| ContactSource {
                    id: source.identifier().to_string(),
                    name: source.name().to_string(),
                })
                .collect())
        })
    }
    fn contacts(&self, selected: &[String]) -> Result<Vec<SystemContact>, String> {
        autoreleasepool(|_| unsafe {
            let store = CNContactStore::new();
            let keys = NSArray::from_slice(&[
                ProtocolObject::from_ref(CNContactIdentifierKey),
                ProtocolObject::from_ref(CNContactGivenNameKey),
                ProtocolObject::from_ref(CNContactFamilyNameKey),
                ProtocolObject::from_ref(CNContactMiddleNameKey),
                ProtocolObject::from_ref(CNContactEmailAddressesKey),
                ProtocolObject::from_ref(CNContactOrganizationNameKey),
                ProtocolObject::from_ref(CNContactJobTitleKey),
            ]);
            let rows = RefCell::new(Vec::new());
            let overflow = RefCell::new(false);
            for source in selected {
                let request = CNContactFetchRequest::initWithKeysToFetch(
                    CNContactFetchRequest::alloc(),
                    &keys,
                );
                request.setUnifyResults(false);
                request.setPredicate(Some(
                    &CNContact::predicateForContactsInContainerWithIdentifier(&NSString::from_str(
                        source,
                    )),
                ));
                let callback = RcBlock::new(
                    |contact: std::ptr::NonNull<CNContact>, stop: std::ptr::NonNull<Bool>| {
                        if rows.borrow().len() >= MAX_SYSTEM_CONTACTS {
                            *overflow.borrow_mut() = true;
                            *stop.as_ptr() = Bool::YES;
                            return;
                        }
                        let contact = contact.as_ref();
                        let name = [
                            contact.givenName().to_string(),
                            contact.middleName().to_string(),
                            contact.familyName().to_string(),
                        ]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                        let company = contact.organizationName().to_string();
                        let role = contact.jobTitle().to_string();
                        rows.borrow_mut().push(SystemContact {
                            id: contact.identifier().to_string(),
                            source_id: source.clone(),
                            display_name: (!name.is_empty()).then_some(name),
                            addresses: contact
                                .emailAddresses()
                                .iter()
                                .map(|value| value.value().to_string())
                                .collect(),
                            company: (!company.is_empty()).then_some(company),
                            role: (!role.is_empty()).then_some(role),
                        });
                    },
                );
                let mut error = None;
                if !store.enumerateContactsWithFetchRequest_error_usingBlock(
                    &request,
                    Some(&mut error),
                    &callback,
                ) {
                    return Err(error
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "Could not read Mac Contacts".into()));
                }
                if *overflow.borrow() {
                    return Err("Too many Mac contacts. Select fewer address books.".into());
                }
            }
            Ok(rows.into_inner())
        })
    }
}
