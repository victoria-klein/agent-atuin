use clap::Subcommand;
use eyre::{Result, WrapErr};
use serde::Serialize;

use atuin_client::{
    database::Database,
    encryption,
    history::store::HistoryStore,
    record::{sqlite_store::SqliteStore, store::Store, sync},
    settings::Settings,
};

mod status;

use crate::command::client::account;

/// JSON output format for sync result
#[derive(Debug, Serialize)]
pub struct SyncResultJson {
    pub status: String,
    pub uploaded: i64,
    pub downloaded: usize,
    pub history_count: i64,
    pub store_history_count: u64,
    pub store_init_triggered: bool,
    pub force: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Sync with the configured server
    Sync {
        /// Force re-download everything
        #[arg(long, short)]
        force: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Login to the configured server
    Login(account::login::Cmd),

    /// Log out
    Logout,

    /// Register with the configured server
    Register(account::register::Cmd),

    /// Print the encryption key for transfer to another machine
    Key {
        /// Switch to base64 output of the key
        #[arg(long)]
        base64: bool,
    },

    /// Display the sync status
    Status,
}

impl Cmd {
    pub async fn run(
        self,
        settings: Settings,
        db: &impl Database,
        store: SqliteStore,
    ) -> Result<()> {
        match self {
            Self::Sync { force, json } => run(&settings, force, json, db, store).await,
            Self::Login(l) => l.run(&settings, &store).await,
            Self::Logout => account::logout::run().await,
            Self::Register(r) => r.run(&settings).await,
            Self::Status => status::run(&settings, db).await,
            Self::Key { base64 } => {
                use atuin_client::encryption::{encode_key, load_key};
                let key = load_key(&settings).wrap_err("could not load encryption key")?;

                if base64 {
                    let encode = encode_key(&key).wrap_err("could not encode encryption key")?;
                    println!("{encode}");
                } else {
                    let mnemonic = bip39::Mnemonic::from_entropy(&key, bip39::Language::English)
                        .map_err(|_| eyre::eyre!("invalid key"))?;
                    println!("{mnemonic}");
                }
                Ok(())
            }
        }
    }
}

async fn run(
    settings: &Settings,
    force: bool,
    json: bool,
    db: &impl Database,
    store: SqliteStore,
) -> Result<()> {
    let mut store_init_triggered = false;
    let mut total_uploaded: i64 = 0;
    let mut total_downloaded: usize = 0;

    if settings.sync.records {
        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();

        let host_id = Settings::host_id().await?;
        let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

        let (uploaded, downloaded) = sync::sync(settings, &store).await?;
        total_uploaded = uploaded;
        total_downloaded = downloaded.len();

        crate::sync::build(settings, &store, db, Some(&downloaded)).await?;

        if !json {
            println!("{uploaded}/{} up/down to record store", downloaded.len());
        }

        let history_length = db.history_count(true).await?;
        let store_history_length = store.len_tag("history").await?;

        #[allow(clippy::cast_sign_loss)]
        if history_length as u64 > store_history_length {
            store_init_triggered = true;

            if !json {
                println!(
                    "{history_length} in history index, but {store_history_length} in history store"
                );
                println!("Running automatic history store init...");
            }

            // Internally we use the global filter mode, so this context is ignored.
            // don't recurse or loop here.
            history_store.init_store(db).await?;

            if !json {
                println!("Re-running sync due to new records locally");
            }

            // we'll want to run sync once more, as there will now be stuff to upload
            let (uploaded, downloaded) = sync::sync(settings, &store).await?;
            total_uploaded += uploaded;
            total_downloaded += downloaded.len();

            crate::sync::build(settings, &store, db, Some(&downloaded)).await?;

            if !json {
                println!("{uploaded}/{} up/down to record store", downloaded.len());
            }
        }
    } else {
        atuin_client::sync::sync(settings, force, db).await?;
    }

    let history_count = db.history_count(true).await?;
    let store_history_count = store.len_tag("history").await?;

    if json {
        let result = SyncResultJson {
            status: "success".to_string(),
            uploaded: total_uploaded,
            downloaded: total_downloaded,
            history_count,
            store_history_count,
            store_init_triggered,
            force,
            error: None,
        };
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!(
            "Sync complete! {} items in history database, force: {}",
            history_count, force
        );
    }

    Ok(())
}
