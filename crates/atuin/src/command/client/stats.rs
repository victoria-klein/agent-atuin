use clap::Parser;
use eyre::Result;
use interim::parse_date_string;
use serde::Serialize;
use time::{Duration, OffsetDateTime, Time};

use atuin_client::{
    database::{Database, current_context},
    settings::Settings,
    theme::Theme,
};

use atuin_history::stats::{compute, pretty_print};

/// JSON output format for stats
#[derive(Debug, Serialize)]
pub struct StatsJson {
    pub period: String,
    pub total_commands: usize,
    pub unique_commands: usize,
    pub top: Vec<StatsTopJson>,
    pub ngram_size: usize,
}

#[derive(Debug, Serialize)]
pub struct StatsTopJson {
    pub command: String,
    pub count: usize,
}

fn parse_ngram_size(s: &str) -> Result<usize, String> {
    let value = s
        .parse::<usize>()
        .map_err(|_| format!("'{s}' is not a valid window size"))?;

    if value == 0 {
        return Err("ngram window size must be at least 1".to_string());
    }

    Ok(value)
}

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
pub struct Cmd {
    /// Compute statistics for the specified period, leave blank for statistics since the beginning. See [this](https://docs.atuin.sh/reference/stats/) for more details.
    period: Vec<String>,

    /// How many top commands to list
    #[arg(long, short, default_value = "10")]
    count: usize,

    /// The number of consecutive commands to consider
    #[arg(long, short, default_value = "1", value_parser = parse_ngram_size)]
    ngram_size: usize,

    /// Output in JSON format
    #[arg(long)]
    json: bool,
}

impl Cmd {
    pub async fn run(&self, db: &impl Database, settings: &Settings, theme: &Theme) -> Result<()> {
        let context = current_context().await?;
        let words = if self.period.is_empty() {
            String::from("all")
        } else {
            self.period.join(" ")
        };

        let now = OffsetDateTime::now_utc().to_offset(settings.timezone.0);
        let last_night = now.replace_time(Time::MIDNIGHT);

        let history = if words.as_str() == "all" {
            db.list(&[], &context, None, false, false).await?
        } else if words.trim() == "today" {
            let start = last_night;
            let end = start + Duration::days(1);
            db.range(start, end).await?
        } else if words.trim() == "month" {
            let end = last_night;
            let start = end - Duration::days(31);
            db.range(start, end).await?
        } else if words.trim() == "week" {
            let end = last_night;
            let start = end - Duration::days(7);
            db.range(start, end).await?
        } else if words.trim() == "year" {
            let end = last_night;
            let start = end - Duration::days(365);
            db.range(start, end).await?
        } else {
            let start = parse_date_string(&words, now, settings.dialect.into())?;
            let end = start + Duration::days(1);
            db.range(start, end).await?
        };

        let stats = compute(settings, &history, self.count, self.ngram_size);

        if let Some(stats) = stats {
            if self.json {
                let json_stats = StatsJson {
                    period: words.clone(),
                    total_commands: stats.total_commands,
                    unique_commands: stats.unique_commands,
                    top: stats
                        .top
                        .iter()
                        .map(|(commands, count)| StatsTopJson {
                            command: commands.join(" | "),
                            count: *count,
                        })
                        .collect(),
                    ngram_size: self.ngram_size,
                };
                println!("{}", serde_json::to_string(&json_stats)?);
            } else {
                pretty_print(stats, self.ngram_size, theme);
            }
        }

        Ok(())
    }
}
