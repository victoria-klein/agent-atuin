use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use clap::Parser;
use eyre::Result;
use indicatif::ProgressBar;
use serde::Serialize;

use atuin_client::{
    database::Database,
    history::History,
    import::{
        Importer, Loader, bash::Bash, fish::Fish, nu::Nu, nu_histdb::NuHistDb,
        powershell::PowerShell, replxx::Replxx, resh::Resh, xonsh::Xonsh,
        xonsh_sqlite::XonshSqlite, zsh::Zsh, zsh_histdb::ZshHistDb,
    },
};

/// JSON output format for import result
#[derive(Debug, Serialize)]
pub struct ImportResultJson {
    pub status: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub entries_found: usize,
    pub entries_imported: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
pub struct Cmd {
    /// Output in JSON format
    #[arg(long)]
    json: bool,

    #[command(subcommand)]
    shell: ShellCmd,
}

#[derive(Parser, Debug)]
pub enum ShellCmd {
    /// Import history for the current shell
    Auto,

    /// Import history from the zsh history file
    Zsh,
    /// Import history from the zsh history file
    ZshHistDb,
    /// Import history from the bash history file
    Bash,
    /// Import history from the replxx history file
    Replxx,
    /// Import history from the resh history file
    Resh,
    /// Import history from the fish history file
    Fish,
    /// Import history from the nu history file
    Nu,
    /// Import history from the nu history file
    NuHistDb,
    /// Import history from xonsh json files
    Xonsh,
    /// Import history from xonsh sqlite db
    XonshSqlite,
    /// Import history from the powershell history file
    Powershell,
}

const BATCH_SIZE: usize = 100;

impl Cmd {
    #[allow(clippy::cognitive_complexity)]
    pub async fn run<DB: Database>(&self, db: &DB) -> Result<()> {
        if !self.json {
            println!("        Atuin         ");
            println!("======================");
            println!("          \u{1f30d}          ");
            println!("       \u{1f418}\u{1f418}\u{1f418}\u{1f418}       ");
            println!("          \u{1f422}          ");
            println!("======================");
            println!("Importing history...");
        }

        let result = match &self.shell {
            ShellCmd::Auto => {
                if cfg!(windows) {
                    return if env::var("PSModulePath").is_ok() {
                        if !self.json {
                            println!("Detected PowerShell");
                        }
                        import::<PowerShell, DB>(db, self.json).await
                    } else {
                        if self.json {
                            let result = ImportResultJson {
                                status: "error".to_string(),
                                source: "unknown".to_string(),
                                source_path: None,
                                entries_found: 0,
                                entries_imported: 0,
                                error: Some("Could not detect the current shell".to_string()),
                            };
                            println!("{}", serde_json::to_string(&result)?);
                        } else {
                            println!("Could not detect the current shell.");
                            println!("Please run atuin import <SHELL>.");
                            println!("To view a list of shells, run atuin import.");
                        }
                        Ok(())
                    };
                }

                // $XONSH_HISTORY_BACKEND isn't always set, but $XONSH_HISTORY_FILE is
                let xonsh_histfile =
                    env::var("XONSH_HISTORY_FILE").unwrap_or_else(|_| String::new());
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("NO_SHELL"));

                if xonsh_histfile.to_lowercase().ends_with(".json") {
                    if !self.json {
                        println!("Detected Xonsh");
                    }
                    import::<Xonsh, DB>(db, self.json).await
                } else if xonsh_histfile.to_lowercase().ends_with(".sqlite") {
                    if !self.json {
                        println!("Detected Xonsh (SQLite backend)");
                    }
                    import::<XonshSqlite, DB>(db, self.json).await
                } else if shell.ends_with("/zsh") {
                    if ZshHistDb::histpath().is_ok() {
                        if !self.json {
                            println!(
                                "Detected Zsh-HistDb, using :{}",
                                ZshHistDb::histpath().unwrap().to_str().unwrap()
                            );
                        }
                        import::<ZshHistDb, DB>(db, self.json).await
                    } else {
                        if !self.json {
                            println!("Detected ZSH");
                        }
                        import::<Zsh, DB>(db, self.json).await
                    }
                } else if shell.ends_with("/fish") {
                    if !self.json {
                        println!("Detected Fish");
                    }
                    import::<Fish, DB>(db, self.json).await
                } else if shell.ends_with("/bash") {
                    if !self.json {
                        println!("Detected Bash");
                    }
                    import::<Bash, DB>(db, self.json).await
                } else if shell.ends_with("/nu") {
                    if NuHistDb::histpath().is_ok() {
                        if !self.json {
                            println!(
                                "Detected Nu-HistDb, using :{}",
                                NuHistDb::histpath().unwrap().to_str().unwrap()
                            );
                        }
                        import::<NuHistDb, DB>(db, self.json).await
                    } else {
                        if !self.json {
                            println!("Detected Nushell");
                        }
                        import::<Nu, DB>(db, self.json).await
                    }
                } else if shell.ends_with("/pwsh") {
                    if !self.json {
                        println!("Detected PowerShell");
                    }
                    import::<PowerShell, DB>(db, self.json).await
                } else {
                    if self.json {
                        let result = ImportResultJson {
                            status: "error".to_string(),
                            source: shell.clone(),
                            source_path: None,
                            entries_found: 0,
                            entries_imported: 0,
                            error: Some(format!("cannot import {shell} history")),
                        };
                        println!("{}", serde_json::to_string(&result)?);
                    } else {
                        println!("cannot import {shell} history");
                    }
                    Ok(())
                }
            }

            ShellCmd::Zsh => import::<Zsh, DB>(db, self.json).await,
            ShellCmd::ZshHistDb => import::<ZshHistDb, DB>(db, self.json).await,
            ShellCmd::Bash => import::<Bash, DB>(db, self.json).await,
            ShellCmd::Replxx => import::<Replxx, DB>(db, self.json).await,
            ShellCmd::Resh => import::<Resh, DB>(db, self.json).await,
            ShellCmd::Fish => import::<Fish, DB>(db, self.json).await,
            ShellCmd::Nu => import::<Nu, DB>(db, self.json).await,
            ShellCmd::NuHistDb => import::<NuHistDb, DB>(db, self.json).await,
            ShellCmd::Xonsh => import::<Xonsh, DB>(db, self.json).await,
            ShellCmd::XonshSqlite => import::<XonshSqlite, DB>(db, self.json).await,
            ShellCmd::Powershell => import::<PowerShell, DB>(db, self.json).await,
        };

        result
    }
}

pub struct HistoryImporter<'db, DB: Database> {
    pb: Option<ProgressBar>,
    buf: Vec<History>,
    db: &'db DB,
    imported_count: Arc<AtomicUsize>,
    json_mode: bool,
}

impl<'db, DB: Database> HistoryImporter<'db, DB> {
    fn new(db: &'db DB, len: usize, json_mode: bool) -> Self {
        Self {
            pb: if json_mode {
                None
            } else {
                Some(ProgressBar::new(len as u64))
            },
            buf: Vec::with_capacity(BATCH_SIZE),
            db,
            imported_count: Arc::new(AtomicUsize::new(0)),
            json_mode,
        }
    }

    fn get_imported_count(&self) -> usize {
        self.imported_count.load(Ordering::SeqCst)
    }

    async fn flush(self) -> Result<usize> {
        if !self.buf.is_empty() {
            self.db.save_bulk(&self.buf).await?;
        }
        if let Some(pb) = &self.pb {
            pb.finish();
        }
        Ok(self.get_imported_count())
    }
}

#[async_trait]
impl<DB: Database> Loader for HistoryImporter<'_, DB> {
    async fn push(&mut self, hist: History) -> Result<()> {
        if let Some(pb) = &self.pb {
            pb.inc(1);
        }
        self.imported_count.fetch_add(1, Ordering::SeqCst);
        self.buf.push(hist);
        if self.buf.len() == self.buf.capacity() {
            self.db.save_bulk(&self.buf).await?;
            self.buf.clear();
        }
        Ok(())
    }
}

async fn import<I: Importer + Send, DB: Database>(db: &DB, json_mode: bool) -> Result<()> {
    if !json_mode {
        println!("Importing history from {}", I::NAME);
    }

    let mut importer = I::new().await?;
    let entries_found = importer.entries().await.unwrap_or(0);
    let mut loader = HistoryImporter::new(db, entries_found, json_mode);
    importer.load(&mut loader).await?;
    let entries_imported = loader.flush().await?;

    if json_mode {
        let result = ImportResultJson {
            status: "success".to_string(),
            source: I::NAME.to_string(),
            source_path: None,
            entries_found,
            entries_imported,
            error: None,
        };
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("Import complete!");
    }

    Ok(())
}
