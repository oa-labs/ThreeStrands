//! Explicit, reversible feedback about a requester's delivery pattern.
//! Header identities always come from the cached message, never UI arguments.

use super::{normalize_sender, Database, DatabaseError, DbResult};
use crate::models::SenderDeliveryPattern;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use threestrands_sync_protocol::EntityType;
use url::Url;

const MAX_LINK_ORIGINS: usize = threestrands_sync_protocol::MAX_SENDER_CONFIRMATION_ORIGINS;

/// Stable across devices, including confirmations made before sync support.
/// The key matches the table's uniqueness constraint; evidence is one atomic
/// replica field so concurrent writes cannot splice unrelated security data.
pub(crate) fn confirmation_id(
    account: &str,
    requester: &str,
    sender: &str,
    origins: &str,
) -> String {
    let key = serde_json::to_vec(&(account, requester, sender, origins)).expect("string tuple");
    format!(
        "sender-confirmation:{}",
        Sha256::digest(key)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SenderConfirmation {
    pub id: String,
    pub account_id: String,
    pub requester: String,
    pub sender_email: String,
    pub reply_to: Option<String>,
    pub link_origins: Vec<String>,
    pub authenticated: bool,
    pub source_message_id: String,
    pub confirmed_at: String,
}

fn confirmation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SenderConfirmation> {
    let origins: String = row.get(5)?;
    Ok(SenderConfirmation {
        id: row.get(0)?,
        account_id: row.get(1)?,
        requester: row.get(2)?,
        sender_email: row.get(3)?,
        reply_to: row.get(4)?,
        link_origins: serde_json::from_str(&origins).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
        })?,
        authenticated: row.get(6)?,
        source_message_id: row.get(7)?,
        confirmed_at: row.get(8)?,
    })
}
const COLUMNS: &str = "id, account_id, requester, sender_email, reply_to, link_origins, authenticated, source_message_id, confirmed_at";

fn normalized_origins(values: &[String]) -> DbResult<Vec<String>> {
    if values.len() > MAX_LINK_ORIGINS {
        return Err(DatabaseError::invalid(
            "Too many link destinations to remember",
        ));
    }
    let mut origins = values
        .iter()
        .map(|value| {
            let url = Url::parse(value)
                .map_err(|_| DatabaseError::invalid("Invalid link destination"))?;
            if !matches!(url.scheme(), "https" | "http")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(DatabaseError::invalid(
                    "Sender feedback stores only web origins",
                ));
            }
            Ok(url.origin().ascii_serialization())
        })
        .collect::<DbResult<Vec<_>>>()?;
    origins.sort();
    origins.dedup();
    Ok(origins)
}

struct DeliveryIdentity {
    account: String,
    sender: String,
    reply_to: Option<String>,
    authenticated: bool,
}

impl DeliveryIdentity {
    fn requester(&self) -> &str {
        self.reply_to.as_deref().unwrap_or(&self.sender)
    }
}

impl Database {
    fn delivery_identity(&self, message_id: &str) -> DbResult<Option<DeliveryIdentity>> {
        let stored = self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT t.account_id, m.sender FROM messages m JOIN threads t ON t.id = m.thread_id WHERE m.id = ?1",
                [message_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            ).optional()?)
        })?;
        let Some((account, sender)) = stored else {
            return Ok(None);
        };
        let Some(trust) = self.message_trust(message_id)? else {
            return Ok(None);
        };
        let sender = normalize_sender(&sender).0;
        if !sender.contains('@')
            || trust.from_domain.is_empty()
            || sender.rsplit_once('@').map(|(_, domain)| domain) != Some(trust.from_domain.as_str())
        {
            return Ok(None);
        }
        Ok(Some(DeliveryIdentity {
            account,
            sender,
            reply_to: trust.reply_to,
            // Only a confirmation made on authenticated mail can teach a
            // reusable pattern. Other confirmations remain message-specific.
            authenticated: trust
                .authentication
                .is_some_and(|auth| auth.dmarc.as_deref() == Some("pass")),
        }))
    }

    pub(super) fn sender_delivery_patterns(
        &self,
        message_id: &str,
    ) -> DbResult<Vec<SenderDeliveryPattern>> {
        let Some(identity) = self.delivery_identity(message_id)? else {
            return Ok(Vec::new());
        };
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, sender_email, reply_to, link_origins, authenticated, source_message_id, confirmed_at
                 FROM sender_delivery_patterns WHERE account_id = ?1 AND requester = ?2 ORDER BY confirmed_at DESC, id",
            )?;
            let patterns = statement.query_map(params![identity.account, identity.requester()], |row| {
                let origins: String = row.get(3)?;
                let link_origins = serde_json::from_str(&origins).map_err(|error| rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error)))?;
                Ok(SenderDeliveryPattern {
                    id: row.get(0)?, sender_email: row.get(1)?, reply_to: row.get(2)?, link_origins,
                    authenticated: row.get(4)?, source_message_id: row.get(5)?, confirmed_at: row.get(6)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok(patterns)
        })
    }

    pub fn confirm_sender_delivery(
        &self,
        message_id: &str,
        link_origins: &[String],
    ) -> DbResult<()> {
        let identity = self.delivery_identity(message_id)?.ok_or_else(|| {
            DatabaseError::invalid("Sender feedback needs this email's stored headers")
        })?;
        let origins = normalized_origins(link_origins)?;
        let origins = serde_json::to_string(&origins).map_err(super::serialization_error)?;
        let id = confirmation_id(
            &identity.account,
            identity.requester(),
            &identity.sender,
            &origins,
        );
        let enabled = self.replicated_sync_active()? && !self.is_projecting_remote_operation();
        self.with_transaction(|connection| {
            connection.execute(
                "INSERT INTO sender_delivery_patterns(id, account_id, requester, sender_email, reply_to, link_origins, authenticated, source_message_id, confirmed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(account_id, requester, sender_email, link_origins) DO NOTHING",
                params![id, identity.account, identity.requester(), identity.sender, identity.reply_to, origins, identity.authenticated, message_id, Utc::now().to_rfc3339()],
            )?;
            if enabled {
                let record = connection.query_row(&format!("SELECT {COLUMNS} FROM sender_delivery_patterns WHERE id=?1"), [&id], confirmation_from_row)?;
                let payload = serde_json::json!({"pattern": record});
                crate::sync_state::record_replicated_write_in_transaction(connection, EntityType::SenderConfirmation, &id, &["pattern".to_string()].into(), &payload)?;
            }
            Ok(())
        })
    }

    pub fn forget_sender_delivery(&self, id: &str) -> DbResult<()> {
        let enabled = self.replicated_sync_active()? && !self.is_projecting_remote_operation();
        self.with_transaction(|connection| {
            connection.execute("DELETE FROM sender_delivery_patterns WHERE id = ?1", [id])?;
            if enabled {
                crate::sync_state::record_replicated_deletion_in_transaction(
                    connection,
                    EntityType::SenderConfirmation,
                    id,
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn list_sender_confirmations(&self) -> DbResult<Vec<SenderConfirmation>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {COLUMNS} FROM sender_delivery_patterns ORDER BY id"
            ))?;
            let records = statement
                .query_map([], confirmation_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        })
    }

    pub(crate) fn upsert_synced_sender_confirmation(
        &self,
        record: &SenderConfirmation,
    ) -> DbResult<()> {
        let normalized = normalized_origins(&record.link_origins)?;
        let origins = serde_json::to_string(&normalized).map_err(super::serialization_error)?;
        let address = |value: &str| {
            value.contains('@')
                && value.len() <= 320
                && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        };
        if !address(&record.account_id)
            || !address(&record.sender_email)
            || !address(&record.requester)
            || record
                .reply_to
                .as_deref()
                .is_some_and(|value| !address(value))
            || record.requester != record.reply_to.as_deref().unwrap_or(&record.sender_email)
            || record.link_origins != normalized
            || record.source_message_id.is_empty()
            || record.source_message_id.len() > 2048
            || chrono::DateTime::parse_from_rfc3339(&record.confirmed_at).is_err()
            || record.id
                != confirmation_id(
                    &record.account_id,
                    &record.requester,
                    &record.sender_email,
                    &origins,
                )
        {
            return Err(DatabaseError::invalid(
                "Invalid synchronized sender confirmation",
            ));
        }
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sender_delivery_patterns(id, account_id, requester, sender_email, reply_to, link_origins, authenticated, source_message_id, confirmed_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(id) DO UPDATE SET authenticated=excluded.authenticated, source_message_id=excluded.source_message_id, confirmed_at=excluded.confirmed_at",
                params![record.id, record.account_id, record.requester, record.sender_email, record.reply_to, origins, record.authenticated, record.source_message_id, record.confirmed_at],
            )?;
            Ok(())
        })
    }
}
