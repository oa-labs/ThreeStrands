//! Standard address-book interchange. Files are bounded, parsed as data, and
//! never cause URL fetches. The native profile validator remains authoritative.
use crate::models::{ContactProfile, SaveContactRequest};
use calcard::{
    vcard::{VCardProperty as Property, VCardValue},
    Entry, Parser,
};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_IMPORT_BYTES: u64 = 10 * 1024 * 1024;
pub(crate) const MAX_IMPORT_CONTACTS: usize = 5_000;
const MAX_WARNINGS: usize = 100;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContactFormat {
    Csv,
    Vcard,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactImportPreview {
    pub contacts: Vec<SaveContactRequest>,
    pub warnings: Vec<String>,
    pub skipped: usize,
}

fn empty() -> SaveContactRequest {
    SaveContactRequest {
        id: None,
        display_name: None,
        role: None,
        company: None,
        location: None,
        bio: None,
        notes: None,
        links: Vec::new(),
        photo_data: None,
        favorite: false,
        addresses: Vec::new(),
        birthday: None,
        keep_in_touch: None,
    }
}
fn optional(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}
fn warn(preview: &mut ContactImportPreview, message: String) {
    if preview.warnings.len() < MAX_WARNINGS {
        preview.warnings.push(message);
    } else {
        preview.warnings[MAX_WARNINGS - 1] =
            "Additional warnings omitted; unsupported fields are not imported.".into();
    }
}
fn normalize_header(header: &str) -> String {
    header
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}
fn email_header(header: &str) -> bool {
    matches!(
        header,
        "email" | "emailaddress" | "primaryemail" | "secondaryemail"
    ) || header.strip_prefix("email").is_some_and(|tail| {
        tail.chars().all(|c| c.is_ascii_digit())
            || tail
                .strip_suffix("value")
                .or_else(|| tail.strip_suffix("address"))
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    })
}

fn accept(
    preview: &mut ContactImportPreview,
    mut contact: SaveContactRequest,
    row: usize,
) -> Result<(), String> {
    contact.addresses = contact
        .addresses
        .into_iter()
        .map(|address| {
            address
                .trim()
                .trim_start_matches("mailto:")
                .to_ascii_lowercase()
        })
        .filter(|address| !address.is_empty())
        .collect();
    let mut unique = std::collections::HashSet::new();
    contact
        .addresses
        .retain(|address| unique.insert(address.clone()));
    if contact.addresses.is_empty()
        || contact.addresses.iter().any(|address| {
            address.len() > 320
                || address.chars().any(char::is_control)
                || crate::correspondence::addresses(address)
                    .map_or(true, |parsed| parsed.len() != 1 || parsed[0].1 != *address)
        })
    {
        preview.skipped += 1;
        warn(
            preview,
            format!("Contact {row}: skipped because it needs valid email addresses."),
        );
        return Ok(());
    }
    if preview.contacts.len() >= MAX_IMPORT_CONTACTS {
        return Err(format!(
            "Import at most {MAX_IMPORT_CONTACTS} contacts at a time"
        ));
    }
    preview.contacts.push(contact);
    Ok(())
}

pub fn parse(data: &str, format: ContactFormat) -> Result<ContactImportPreview, String> {
    if data.len() as u64 > MAX_IMPORT_BYTES {
        return Err("Contact files must be at most 10 MiB".into());
    }
    let data = data.trim_start_matches('\u{feff}');
    let mut preview = ContactImportPreview {
        contacts: Vec::new(),
        warnings: Vec::new(),
        skipped: 0,
    };
    match format {
        ContactFormat::Csv => {
            let mut reader = csv::ReaderBuilder::new()
                .flexible(false)
                .from_reader(data.as_bytes());
            let headers = reader
                .headers()
                .map_err(|error| format!("Invalid CSV header: {error}"))?
                .iter()
                .map(normalize_header)
                .collect::<Vec<_>>();
            if !headers.iter().any(|header| email_header(header)) {
                return Err(
                    "CSV needs an Email column or Google Contacts E-mail 1 - Value columns".into(),
                );
            }
            let mut ignored = std::collections::BTreeSet::new();
            for (index, row) in reader.records().enumerate() {
                if index >= MAX_IMPORT_CONTACTS {
                    return Err(format!(
                        "Import at most {MAX_IMPORT_CONTACTS} contacts at a time"
                    ));
                }
                let row =
                    row.map_err(|error| format!("Invalid CSV record {}: {error}", index + 2))?;
                let mut contact = empty();
                let mut given = String::new();
                let mut family = String::new();
                for (header, value) in headers.iter().zip(row.iter()) {
                    if value.trim().is_empty() {
                        continue;
                    }
                    if email_header(header) {
                        contact
                            .addresses
                            .extend(value.split(" ::: ").map(str::to_string));
                        continue;
                    }
                    match header.as_str() {
                        "name" | "displayname" | "fullname" => {
                            contact.display_name = optional(value)
                        }
                        "givenname" | "firstname" => given = value.to_string(),
                        "familyname" | "lastname" => family = value.to_string(),
                        "role" | "title" | "jobtitle" | "organization1title" => {
                            contact.role = optional(value)
                        }
                        "company" | "organization" | "organization1name" => {
                            contact.company = optional(value)
                        }
                        "location" => contact.location = optional(value),
                        "bio" | "about" => contact.bio = optional(value),
                        "notes" | "note" => contact.notes = optional(value),
                        "birthday" => contact.birthday = optional(value),
                        "links" | "website" | "website1value" => {
                            contact.links.extend(value.lines().filter_map(optional))
                        }
                        "favorite" => {
                            contact.favorite = matches!(
                                value.trim().to_ascii_lowercase().as_str(),
                                "true" | "yes" | "1"
                            )
                        }
                        _ => {
                            ignored.insert(header.clone());
                        }
                    }
                }
                if contact.display_name.is_none() {
                    contact.display_name = optional(&format!("{given} {family}"));
                }
                accept(&mut preview, contact, index + 1)?;
            }
            if !ignored.is_empty() {
                warn(
                    &mut preview,
                    format!(
                        "Unsupported CSV fields are omitted: {}.",
                        ignored.into_iter().collect::<Vec<_>>().join(", ")
                    ),
                );
            }
        }
        ContactFormat::Vcard => {
            let mut parser = Parser::new(data).strict();
            let mut count = 0;
            loop {
                let card =
                    match parser.entry() {
                        Entry::VCard(card) => card,
                        Entry::Eof => break,
                        _ => return Err(
                            "Invalid vCard file; expected complete BEGIN:VCARD / END:VCARD records"
                                .into(),
                        ),
                    };
                count += 1;
                if count > MAX_IMPORT_CONTACTS {
                    return Err(format!(
                        "Import at most {MAX_IMPORT_CONTACTS} contacts at a time"
                    ));
                }
                let mut contact = empty();
                let mut has_photo = false;
                let mut unsupported = std::collections::BTreeSet::new();
                let mut emails = Vec::new();
                for entry in &card.entries {
                    let text = entry
                        .values
                        .iter()
                        .filter_map(VCardValue::as_text)
                        .collect::<Vec<_>>()
                        .join(" ");
                    match &entry.name {
                        Property::Fn => contact.display_name = optional(&text),
                        Property::N if contact.display_name.is_none() => {
                            contact.display_name = optional(
                                &[3, 1, 2, 0, 4]
                                    .into_iter()
                                    .filter_map(|index| {
                                        entry.values.get(index).and_then(VCardValue::as_text)
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            )
                        }
                        Property::Email => {
                            let pref = entry
                                .parameters(&calcard::vcard::VCardParameterName::Pref)
                                .find_map(|v| {
                                    if let calcard::vcard::VCardParameterValue::Integer(value) = v {
                                        Some(*value)
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or(100);
                            let legacy_preferred = entry
                                .parameters(&calcard::vcard::VCardParameterName::Type)
                                .any(|value| {
                                    value
                                        .as_text()
                                        .is_some_and(|value| value.eq_ignore_ascii_case("pref"))
                                });
                            emails.push((if legacy_preferred { 1 } else { pref }, text));
                        }
                        Property::Title | Property::Role => contact.role = optional(&text),
                        Property::Org => contact.company = optional(&text),
                        Property::Note => contact.notes = optional(&text),
                        Property::Url => {
                            if let Some(link) = optional(&text) {
                                contact.links.push(link);
                            }
                        }
                        Property::Bday => {
                            if let Some(date) = entry
                                .values
                                .first()
                                .and_then(VCardValue::as_partial_date_time)
                            {
                                if let (Some(month), Some(day)) = (date.month, date.day) {
                                    contact.birthday = Some(match date.year {
                                        Some(year) => format!("{year:04}-{month:02}-{day:02}"),
                                        None => format!("{month:02}-{day:02}"),
                                    });
                                }
                            }
                        }
                        Property::Photo => has_photo = true,
                        Property::Other(name)
                            if name.eq_ignore_ascii_case("X-THREESTRANDS-LOCATION") =>
                        {
                            contact.location = optional(&text)
                        }
                        Property::Other(name)
                            if name.eq_ignore_ascii_case("X-THREESTRANDS-BIO") =>
                        {
                            contact.bio = optional(&text)
                        }
                        Property::Other(name)
                            if name.eq_ignore_ascii_case("X-THREESTRANDS-FAVORITE") =>
                        {
                            contact.favorite = text == "true"
                        }
                        Property::Begin
                        | Property::End
                        | Property::Version
                        | Property::Uid
                        | Property::Rev
                        | Property::Prodid
                        | Property::N => {}
                        _ => {
                            unsupported.insert(entry.name.as_str().to_string());
                        }
                    }
                }
                emails.sort_by_key(|(pref, _)| *pref);
                contact.addresses = emails.into_iter().map(|(_, email)| email).collect();
                if has_photo {
                    warn(
                        &mut preview,
                        format!("Contact {count}: photo omitted; no remote images are fetched."),
                    );
                }
                if !unsupported.is_empty() {
                    warn(
                        &mut preview,
                        format!(
                            "Contact {count}: unsupported fields omitted: {}.",
                            unsupported.into_iter().collect::<Vec<_>>().join(", ")
                        ),
                    );
                }
                accept(&mut preview, contact, count)?;
            }
        }
    }
    if preview.contacts.is_empty() && preview.skipped == 0 {
        return Err("The file contains no contacts".into());
    }
    // Reject unsupported field values before offering a confirmation. Native
    // persistence validates every accepted record again inside its transaction.
    for (index, contact) in preview.contacts.iter().enumerate() {
        validate_import_fields(contact)
            .map_err(|error| format!("Contact {}: {error}", index + 1))?;
    }
    Ok(preview)
}

fn validate_import_fields(contact: &SaveContactRequest) -> Result<(), String> {
    for (value, max) in [
        (&contact.display_name, 200),
        (&contact.role, 200),
        (&contact.company, 200),
        (&contact.location, 200),
        (&contact.bio, 4000),
        (&contact.notes, 8000),
    ] {
        if value
            .as_ref()
            .is_some_and(|value| value.chars().count() > max)
        {
            return Err("A contact field is too long".into());
        }
    }
    if contact.links.len() > 20
        || contact.links.iter().any(|link| {
            url::Url::parse(link).map_or(true, |url| {
                url.scheme() != "https" || url.host_str().is_none() || link.len() > 2048
            })
        })
    {
        return Err("Contacts support up to 20 valid HTTPS links".into());
    }
    if let Some(birthday) = &contact.birthday {
        crate::db::contacts::normalize_birthday(Some(birthday)).map_err(String::from)?;
    }
    Ok(())
}

// Spreadsheet applications can execute formula-looking CSV cells. Prefixing
// those cells with an apostrophe makes an exported contact inert on opening.
fn csv_cell(value: String) -> String {
    if value.starts_with(['\t', '\r', '\n']) || value.trim_start().starts_with(['=', '+', '-', '@'])
    {
        format!("'{value}")
    } else {
        value
    }
}
fn escaped(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\\n")
        .replace(';', "\\;")
        .replace(',', "\\,")
}
fn line(output: &mut String, value: &str) {
    let mut bytes = 0;
    for character in value.chars() {
        if bytes + character.len_utf8() > 75 {
            output.push_str("\r\n ");
            bytes = 1;
        }
        output.push(character);
        bytes += character.len_utf8();
    }
    output.push_str("\r\n");
}

pub fn export(contacts: &[ContactProfile], format: ContactFormat) -> Result<Vec<u8>, String> {
    match format {
        ContactFormat::Csv => {
            let count = contacts
                .iter()
                .map(|contact| contact.addresses.len())
                .max()
                .unwrap_or(1)
                .max(1);
            let mut writer = csv::Writer::from_writer(Vec::new());
            let mut headers = vec![
                "Name".into(),
                "Role".into(),
                "Company".into(),
                "Location".into(),
                "Bio".into(),
                "Notes".into(),
                "Birthday".into(),
                "Links".into(),
                "Favorite".into(),
            ];
            headers.extend((1..=count).map(|index| format!("E-mail {index} - Value")));
            writer
                .write_record(headers)
                .map_err(|error| error.to_string())?;
            for contact in contacts {
                let mut row = [
                    &contact.display_name,
                    &contact.role,
                    &contact.company,
                    &contact.location,
                    &contact.bio,
                    &contact.notes,
                    &contact.birthday,
                ]
                .map(|value| value.clone().unwrap_or_default())
                .to_vec();
                row.push(contact.links.join("\n"));
                row.push(contact.favorite.to_string());
                row.extend(
                    (0..count)
                        .map(|index| contact.addresses.get(index).cloned().unwrap_or_default()),
                );
                writer
                    .write_record(row.into_iter().map(csv_cell))
                    .map_err(|error| error.to_string())?;
            }
            writer.into_inner().map_err(|error| error.to_string())
        }
        ContactFormat::Vcard => {
            let mut output = String::new();
            for contact in contacts {
                line(&mut output, "BEGIN:VCARD");
                line(&mut output, "VERSION:4.0");
                line(
                    &mut output,
                    &format!(
                        "FN:{}",
                        escaped(
                            contact
                                .display_name
                                .as_deref()
                                .unwrap_or(&contact.addresses[0])
                        )
                    ),
                );
                for (index, email) in contact.addresses.iter().enumerate() {
                    line(
                        &mut output,
                        &format!(
                            "EMAIL{}:{}",
                            if index == 0 { ";PREF=1" } else { "" },
                            escaped(email)
                        ),
                    );
                }
                for (property, value) in [
                    ("TITLE", &contact.role),
                    ("ORG", &contact.company),
                    ("NOTE", &contact.notes),
                    ("X-THREESTRANDS-LOCATION", &contact.location),
                    ("X-THREESTRANDS-BIO", &contact.bio),
                ] {
                    if let Some(value) = value {
                        line(&mut output, &format!("{property}:{}", escaped(value)));
                    }
                }
                if let Some(birthday) = &contact.birthday {
                    line(
                        &mut output,
                        &format!(
                            "BDAY:{}",
                            if birthday.len() == 5 {
                                format!("--{}", birthday.replace('-', ""))
                            } else {
                                birthday.replace('-', "")
                            }
                        ),
                    );
                }
                for link in &contact.links {
                    // Serialize a URI, not raw saved text: the URL parser
                    // removes CR/LF so a link cannot inject vCard properties.
                    let uri = url::Url::parse(link).map_err(|error| error.to_string())?;
                    line(&mut output, &format!("URL:{uri}"));
                }
                line(
                    &mut output,
                    &format!("X-THREESTRANDS-FAVORITE:{}", contact.favorite),
                );
                line(&mut output, "END:VCARD");
            }
            Ok(output.into_bytes())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn csv_supports_quoted_multiline_fields_google_headers_and_invalid_row_warnings() {
        let result = parse("\u{feff}Name,E-mail 1 - Value,E-mail 2 - Value,Notes,Phone\r\n\"Doe, Jane\",JANE@example.com,other@example.com,\"First line\nSecond line\",555\r\nBad,not-an-email,,,\r\n", ContactFormat::Csv).unwrap();
        assert_eq!(result.contacts.len(), 1);
        assert_eq!(
            result.contacts[0].display_name.as_deref(),
            Some("Doe, Jane")
        );
        assert_eq!(
            result.contacts[0].addresses,
            ["jane@example.com", "other@example.com"]
        );
        assert_eq!(
            result.contacts[0].notes.as_deref(),
            Some("First line\nSecond line")
        );
        assert_eq!(result.skipped, 1);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("phone")));
        assert!(parse(
            "Name,Email\nJane,jane@example.com,extra",
            ContactFormat::Csv
        )
        .is_err());
        assert!(parse("Name,Phone\nJane,555", ContactFormat::Csv).is_err());
    }

    #[test]
    fn vcard_reads_multiple_cards_folding_escaping_preferences_and_birthdays_without_fetches() {
        let data = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Jane\\, Doe\r\nEMAIL:other@example.com\r\nEMAIL;PREF=1:jane@example.com\r\nNOTE:One\\nTwo very long\r\n  lines\r\nBDAY:1984-12-09\r\nPHOTO;VALUE=URI:https://example.com/photo.jpg\r\nTEL:555\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:4.0\r\nFN:Person Two\r\nEMAIL:two@example.com\r\nBDAY:--0229\r\nEND:VCARD\r\n";
        let result = parse(data, ContactFormat::Vcard).unwrap();
        assert_eq!(result.contacts.len(), 2);
        assert_eq!(
            result.contacts[0].display_name.as_deref(),
            Some("Jane, Doe")
        );
        assert_eq!(
            result.contacts[0].addresses,
            ["jane@example.com", "other@example.com"]
        );
        assert_eq!(
            result.contacts[0].notes.as_deref(),
            Some("One\nTwo very long lines")
        );
        assert_eq!(result.contacts[0].birthday.as_deref(), Some("1984-12-09"));
        assert_eq!(result.contacts[1].birthday.as_deref(), Some("02-29"));
        assert!(result.contacts[0].photo_data.is_none());
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("photo omitted")));
        assert!(parse(
            "BEGIN:VCARD\nVERSION:4.0\nEMAIL:jane@example.com\n",
            ContactFormat::Vcard
        )
        .is_err());
        assert!(parse("not a card", ContactFormat::Vcard).is_err());
    }

    #[test]
    fn both_exports_round_trip_supported_fields_and_vcard_folds_utf8_safely() {
        let db = Database::open_memory();
        let mut contact = empty();
        contact.display_name = Some("Zoë, 李".repeat(20));
        contact.addresses = vec!["primary@example.com".into(), "second@example.com".into()];
        contact.role = Some("Engineer".into());
        contact.company = Some("One; Two".into());
        contact.notes = Some("One\nTwo, three; \\four".into());
        contact.bio = Some("Bio".into());
        contact.location = Some("Paris".into());
        contact.birthday = Some("02-29".into());
        contact.links = vec!["https://example.com/a,b?q=1".into()];
        contact.favorite = true;
        let profile = db.save_contact_profile(&contact).unwrap();
        for format in [ContactFormat::Csv, ContactFormat::Vcard] {
            let bytes = export(&[profile.clone()], format).unwrap();
            let data = String::from_utf8(bytes).unwrap();
            if matches!(format, ContactFormat::Vcard) {
                assert!(data.split("\r\n").all(|line| line.len() <= 75));
            }
            let result = parse(&data, format).unwrap();
            let imported = &result.contacts[0];
            assert_eq!(imported.display_name, contact.display_name);
            assert_eq!(imported.addresses, contact.addresses);
            assert_eq!(imported.notes, contact.notes);
            assert_eq!(imported.company, contact.company);
            assert_eq!(imported.birthday, contact.birthday);
            assert_eq!(imported.bio, contact.bio);
            assert_eq!(imported.location, contact.location);
            assert_eq!(imported.links, contact.links);
            assert!(imported.favorite);
        }
    }

    #[test]
    fn formula_cells_are_inert_and_unsafe_links_are_rejected() {
        for value in [
            "=HYPERLINK(\"https://bad\")",
            " +1",
            "-1",
            "@SUM(1)",
            "\tdata",
            "\rdata",
            "\ndata",
        ] {
            assert_eq!(csv_cell(value.into()), format!("'{value}"));
        }
        assert_eq!(csv_cell("ordinary".into()), "ordinary");
        for link in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "http://example.com",
        ] {
            assert!(parse(
                &format!("Name,Email,Website\nJane,jane@example.com,{link}"),
                ContactFormat::Csv
            )
            .is_err());
        }
        assert!(parse(
            "Name,Email,Birthday\nJane,jane@example.com,02-30",
            ContactFormat::Csv
        )
        .is_err());
    }

    #[test]
    fn vcard_export_keeps_multiline_fields_and_uri_controls_inside_one_record() {
        let db = Database::open_memory();
        let mut contact = empty();
        contact.addresses = vec!["person@example.com".into()];
        contact.notes = Some("Note\r\nBEGIN:VCARD\nEMAIL:injected@example.com".into());
        contact.links = vec!["https://example.com/path\r\nX-INJECTED:yes".into()];
        let profile = db.save_contact_profile(&contact).unwrap();
        let data = String::from_utf8(export(&[profile], ContactFormat::Vcard).unwrap()).unwrap();
        let preview = parse(&data, ContactFormat::Vcard).unwrap();
        assert_eq!(preview.contacts.len(), 1);
        assert_eq!(preview.contacts[0].addresses, contact.addresses);
        assert!(!data.contains("\r\nX-INJECTED:"));
        assert!(preview.warnings.is_empty());
    }

    #[test]
    fn import_limits_accept_exact_limits_and_reject_above() {
        for count in [
            MAX_IMPORT_CONTACTS - 1,
            MAX_IMPORT_CONTACTS,
            MAX_IMPORT_CONTACTS + 1,
        ] {
            let data = format!("Name,Email\n{}", "Jane,jane@example.com\n".repeat(count));
            assert_eq!(
                parse(&data, ContactFormat::Csv).is_ok(),
                count <= MAX_IMPORT_CONTACTS
            );
        }
        for size in [MAX_IMPORT_BYTES - 1, MAX_IMPORT_BYTES, MAX_IMPORT_BYTES + 1] {
            // Padding after a valid record remains a legal empty CSV field;
            // no accepted profile field grows with the input size.
            let mut data = "Name,Email,Unused\nJane,jane@example.com,".to_string();
            data.push_str(&" ".repeat(size as usize - data.len()));
            assert_eq!(
                parse(&data, ContactFormat::Csv).is_ok(),
                size <= MAX_IMPORT_BYTES
            );
        }
    }
}
