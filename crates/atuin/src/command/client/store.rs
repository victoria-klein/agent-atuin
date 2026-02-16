use clap::Subcommand;
use eyre::Result;
use serde::Serialize;

use atuin_client::{
    database::Database,
    record::{sqlite_store::SqliteStore, store::Store},
    settings::Settings,
};
use itertools::Itertools;
use time::{OffsetDateTime, UtcOffset};

/// JSON output format for store status
#[derive(Debug, Serialize)]
pub struct StoreStatusJson {
    pub hosts: Vec<StoreHostJson>,
}

#[derive(Debug, Serialize)]
pub struct StoreHostJson {
    pub id: String,
    pub is_current: bool,
    pub tags: Vec<StoreTagJson>,
}

#[derive(Debug, Serialize)]
pub struct StoreTagJson {
    pub name: String,
    pub idx: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<StoreRecordJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<StoreRecordJson>,
}

#[derive(Debug, Serialize)]
pub struct StoreRecordJson {
    pub id: String,
    pub created: String,
}

#[cfg(feature = "sync")]
mod push;

#[cfg(feature = "sync")]
mod pull;

mod purge;
mod rebuild;
mod rekey;
mod verify;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Print the current status of the record store
    Status {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Rebuild a store (eg atuin store rebuild history)
    Rebuild(rebuild::Rebuild),

    /// Re-encrypt the store with a new key (potential for data loss!)
    Rekey(rekey::Rekey),

    /// Delete all records in the store that cannot be decrypted with the current key
    Purge(purge::Purge),

    /// Verify that all records in the store can be decrypted with the current key
    Verify(verify::Verify),

    /// Push all records to the remote sync server (one way sync)
    #[cfg(feature = "sync")]
    Push(push::Push),

    /// Pull records from the remote sync server (one way sync)
    #[cfg(feature = "sync")]
    Pull(pull::Pull),
}

impl Cmd {
    pub async fn run(
        &self,
        settings: &Settings,
        database: &dyn Database,
        store: SqliteStore,
    ) -> Result<()> {
        match self {
            Self::Status { json } => self.status(store, *json).await,
            Self::Rebuild(rebuild) => rebuild.run(settings, store, database).await,
            Self::Rekey(rekey) => rekey.run(settings, store).await,
            Self::Verify(verify) => verify.run(settings, store).await,
            Self::Purge(purge) => purge.run(settings, store).await,

            #[cfg(feature = "sync")]
            Self::Push(push) => push.run(settings, store).await,

            #[cfg(feature = "sync")]
            Self::Pull(pull) => pull.run(settings, store, database).await,
        }
    }

    pub async fn status(&self, store: SqliteStore, json: bool) -> Result<()> {
        let host_id = Settings::host_id().await?;
        let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

        let status = store.status().await?;

        if json {
            let mut hosts_json = Vec::new();

            for (host, st) in status.hosts.iter().sorted_by_key(|(h, _)| *h) {
                let is_current = host == &host_id;
                let mut tags_json = Vec::new();

                for (tag, idx) in st.iter().sorted_by_key(|(tag, _)| *tag) {
                    let first = store.first(*host, tag).await?;
                    let last = store.last(*host, tag).await?;

                    let first_json = if let Some(first) = first {
                        let time =
                            OffsetDateTime::from_unix_timestamp_nanos(i128::from(first.timestamp))?;
                        Some(StoreRecordJson {
                            id: first.id.0.as_hyphenated().to_string(),
                            created: time
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap_or_default(),
                        })
                    } else {
                        None
                    };

                    let last_json = if let Some(last) = last {
                        let time =
                            OffsetDateTime::from_unix_timestamp_nanos(i128::from(last.timestamp))?;
                        Some(StoreRecordJson {
                            id: last.id.0.as_hyphenated().to_string(),
                            created: time
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap_or_default(),
                        })
                    } else {
                        None
                    };

                    tags_json.push(StoreTagJson {
                        name: tag.clone(),
                        idx: *idx,
                        first: first_json,
                        last: last_json,
                    });
                }

                hosts_json.push(StoreHostJson {
                    id: host.0.as_hyphenated().to_string(),
                    is_current,
                    tags: tags_json,
                });
            }

            let result = StoreStatusJson { hosts: hosts_json };
            println!("{}", serde_json::to_string(&result)?);
        } else {
            for (host, st) in status.hosts.iter().sorted_by_key(|(h, _)| *h) {
                let host_string = if host == &host_id {
                    format!("host: {} <- CURRENT HOST", host.0.as_hyphenated())
                } else {
                    format!("host: {}", host.0.as_hyphenated())
                };

                println!("{host_string}");

                for (tag, idx) in st.iter().sorted_by_key(|(tag, _)| *tag) {
                    println!("\tstore: {tag}");

                    let first = store.first(*host, tag).await?;
                    let last = store.last(*host, tag).await?;

                    println!("\t\tidx: {idx}");

                    if let Some(first) = first {
                        println!("\t\tfirst: {}", first.id.0.as_hyphenated());

                        let time =
                            OffsetDateTime::from_unix_timestamp_nanos(i128::from(first.timestamp))?
                                .to_offset(offset);
                        println!("\t\t\tcreated: {time}");
                    }

                    if let Some(last) = last {
                        println!("\t\tlast: {}", last.id.0.as_hyphenated());

                        let time =
                            OffsetDateTime::from_unix_timestamp_nanos(i128::from(last.timestamp))?
                                .to_offset(offset);
                        println!("\t\t\tcreated: {time}");
                    }
                }

                println!();
            }
        }

        Ok(())
    }
}
