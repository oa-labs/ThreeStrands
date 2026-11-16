//! Developer-only evaluator. No live network access without --live and an env key.
use threestrands_lib::ai_evaluation::{self, Options, Report};
#[tokio::main]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn execute() -> Result<(), String> {
    let mut options = Options {
        live: false,
        provider: "custom".into(),
        model: "synthetic-replay".into(),
        endpoint: Some("http://127.0.0.1:1".into()),
        api_key: None,
        case_filter: None,
    };
    let mut output = None;
    let mut comparison = None;
    let mut model_set = false;
    let mut provider_set = false;
    let mut endpoint_set = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--live" => options.live = true,
            "--provider" => {
                options.provider = args.next().ok_or("Missing provider")?;
                provider_set = true;
            }
            "--model" => {
                options.model = args.next().ok_or("Missing model")?;
                model_set = true;
            }
            "--endpoint" => {
                options.endpoint = Some(args.next().ok_or("Missing endpoint")?);
                endpoint_set = true;
            }
            "--case" => options.case_filter = Some(args.next().ok_or("Missing case filter")?),
            "--output" => output = Some(args.next().ok_or("Missing output path")?),
            "--compare" => comparison = Some(args.next().ok_or("Missing comparison report")?),
            "--help" => {
                println!("ai-eval [--case FILTER] [--output REPORT.json]\nai-eval --live --provider PROVIDER --model MODEL [--endpoint URL] [--case FILTER] [--output REPORT.json] [--compare BASELINE.json]\nLive runs send only checked-in synthetic cases and require THREESTRANDS_EVAL_API_KEY. They may be billed. Reports never include provider output. Replay checks wiring, not model quality.");
                return Ok(());
            }
            _ => return Err(format!("Unknown argument: {arg}")),
        }
    }
    if options.live {
        if !model_set || !provider_set {
            return Err("Live runs require explicit --provider and --model".into());
        }
        if options.provider == "custom" && !endpoint_set {
            return Err("Custom live evaluations require an explicit --endpoint".into());
        }
        options.api_key = std::env::var("THREESTRANDS_EVAL_API_KEY").ok();
        if options.provider != "custom" {
            options.endpoint = None;
        }
    }
    let previous = if let Some(path) = comparison {
        let previous: Report = serde_json::from_slice(
            &std::fs::read(path).map_err(|_| "Cannot read comparison report")?,
        )
        .map_err(|_| "Invalid comparison report")?;
        if !options.live
            || previous.mode != "live"
            || previous.suite_version != ai_evaluation::SUITE_VERSION
            || previous.grader_version != ai_evaluation::GRADER_VERSION
            || previous.suite_revision
                != ai_evaluation::suite_revision(options.case_filter.as_deref())?
        {
            return Err("The baseline is not comparable; no live requests were sent".into());
        }
        Some(previous)
    } else {
        None
    };
    let report = ai_evaluation::run(options).await?;
    let comparison = previous
        .as_ref()
        .map(|previous| ai_evaluation::compare(previous, &report))
        .transpose()?;
    let json = serde_json::to_string_pretty(&report).map_err(|_| "Cannot encode report")?;
    if let Some(path) = output {
        std::fs::write(path, &json).map_err(|_| "Cannot write report")?;
    } else {
        println!("{json}");
    }
    if let Some(comparison) = comparison {
        eprintln!("{comparison}");
    }
    if report.failed > 0 {
        return Err(format!("{} evaluation cases failed", report.failed));
    }
    Ok(())
}
