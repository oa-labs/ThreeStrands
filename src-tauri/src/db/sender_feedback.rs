//! Explicit, reversible feedback about a requester's delivery pattern.
//! Header identities always come from the cached message, never UI arguments.

use super::{normalize_sender, Database, DatabaseError, DbResult};
use crate::models::SenderDeliveryPattern;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use url::Url;
use uuid::Uuid;

const MAX_LINK_ORIGINS: usize = 500;

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
        if link_origins.len() > MAX_LINK_ORIGINS {
            return Err(DatabaseError::invalid(
                "Too many link destinations to remember",
            ));
        }
        let mut origins = link_origins
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
        let origins = serde_json::to_string(&origins)
            .map_err(|error| DatabaseError::Serialization(error.to_string()))?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sender_delivery_patterns(id, account_id, requester, sender_email, reply_to, link_origins, authenticated, source_message_id, confirmed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(account_id, requester, sender_email, link_origins) DO NOTHING",
                params![Uuid::new_v4().to_string(), identity.account, identity.requester(), identity.sender, identity.reply_to, origins, identity.authenticated, message_id, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn forget_sender_delivery(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sender_delivery_patterns WHERE id = ?1", [id])?;
            Ok(())
        })
    }
}
