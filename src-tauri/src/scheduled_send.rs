//! Local scheduling authority is never reconstructed from replicated summaries.
use crate::{
    correspondence::{build_mime, OutboxItem},
    db::Database,
};
use chrono::{NaiveDateTime, Offset, TimeZone};
use serde::{Deserialize, Serialize};
use std::path::Path;
pub use threestrands_sync_protocol::ScheduledSendReport;

pub const DISPATCH_GRACE_MS: i64 = 60_000;
pub type SendingIdentity = dyn Fn(bool) -> Result<String, String> + Send + Sync;

/// `create` is false whenever a persisted job needs its original identity.
pub fn keychain_identity(create: bool) -> Result<String, String> {
    let entry = keyring::Entry::new("app.threestrands.sending-device", "installation")
        .map_err(|_| "Unable to access the sending-device keychain entry")?;
    match entry.get_password() {
        Ok(value) if uuid::Uuid::parse_str(&value).is_ok() => Ok(value),
        Ok(_) => Err("The sending-device identity is damaged; scheduled mail is paused".into()),
        Err(keyring::Error::NoEntry) if create => {
            let id = uuid::Uuid::new_v4().to_string();
            entry
                .set_password(&id)
                .map_err(|_| "Unable to save the sending-device identity")?;
            Ok(id)
        }
        Err(_) => {
            Err("The sending-device identity is unavailable; scheduled mail is paused".into())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub report: ScheduledSendReport,
    pub visibility_group_id: Option<String>,
    #[serde(default)]
    pub can_manage: bool,
    #[serde(default)]
    pub visibility: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub local_time: String,
    pub time_zone: String,
    pub offset_seconds: Option<i32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeChoice {
    pub scheduled_at: i64,
    pub offset_seconds: i32,
}

pub fn choices(local_time: &str, time_zone: &str) -> Result<Vec<TimeChoice>, String> {
    let tz: chrono_tz::Tz = time_zone
        .parse()
        .map_err(|_| "Choose a valid IANA timezone")?;
    let local = NaiveDateTime::parse_from_str(local_time, "%Y-%m-%dT%H:%M")
        .map_err(|_| "Choose a valid date and time")?;
    let values = match tz.from_local_datetime(&local) {
        chrono::LocalResult::None => {
            return Err(
                "That time does not exist because the clocks change. Choose another time.".into(),
            )
        }
        chrono::LocalResult::Single(t) => vec![t],
        chrono::LocalResult::Ambiguous(a, b) => vec![a, b],
    };
    Ok(values
        .into_iter()
        .map(|t| TimeChoice {
            scheduled_at: t.timestamp_millis(),
            offset_seconds: t.offset().fix().local_minus_utc(),
        })
        .collect())
}

pub fn resolve(selection: &Selection, at: i64) -> Result<i64, String> {
    let options = choices(&selection.local_time, &selection.time_zone)?;
    let choice = if options.len() == 1 && selection.offset_seconds.is_none() {
        options.first()
    } else {
        options
            .iter()
            .find(|v| Some(v.offset_seconds) == selection.offset_seconds)
    }
    .ok_or("This time occurs twice. Choose its UTC offset.")?;
    if choice.scheduled_at <= at {
        return Err("Choose a time in the future".into());
    }
    Ok(choice.scheduled_at)
}

impl Database {
    #[allow(clippy::too_many_arguments)]
    pub fn schedule_draft(
        &self,
        id: &str,
        revision: i64,
        selection: &Selection,
        owner: &str,
        identities: &[crate::provider::SendingIdentity],
        message_id_domain: Option<&str>,
        root: &Path,
        at: i64,
    ) -> Result<OutboxItem, String> {
        if let Some(item) = self
            .outbox()?
            .into_iter()
            .find(|o| o.draft.id == id && o.draft.revision == revision)
        {
            return Ok(item);
        }
        let scheduled_at = resolve(selection, at)?;
        let draft = self.draft(id)?;
        if draft.revision != revision {
            return Err("Draft is still saving".into());
        }
        let sender_name = self
            .get_account(&draft.account)?
            .and_then(|a| a.display_name);
        let operation_id = uuid::Uuid::new_v4().to_string();
        let raw = build_mime(
            &draft,
            identities,
            sender_name.as_deref(),
            message_id_domain,
            &operation_id,
            root,
        )?;
        self.insert_scheduled_draft(
            &draft,
            revision,
            &operation_id,
            &raw,
            owner,
            scheduled_at,
            &selection.time_zone,
            at,
        )?;
        self.outbox()?
            .into_iter()
            .find(|o| o.id == operation_id)
            .ok_or("Scheduled message missing".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timezone_gap_overlap_offsets_invalid_dates_and_future_validation() {
        assert!(choices("2026-03-08T02:30", "America/New_York").is_err());
        let overlap = choices("2026-11-01T01:30", "America/New_York").unwrap();
        assert_eq!(overlap.len(), 2);
        assert_eq!(overlap[1].scheduled_at - overlap[0].scheduled_at, 3_600_000);
        let mut s = Selection {
            local_time: "2026-11-01T01:30".into(),
            time_zone: "America/New_York".into(),
            offset_seconds: None,
        };
        assert!(resolve(&s, 0).is_err());
        for choice in overlap {
            s.offset_seconds = Some(choice.offset_seconds);
            assert_eq!(resolve(&s, 0).unwrap(), choice.scheduled_at);
            assert!(resolve(&s, choice.scheduled_at).is_err());
        }
        assert!(choices("2026-02-30T12:00", "UTC").is_err());
        assert!(choices("2026-11-01T12:00", "invalid").is_err());
    }
}
