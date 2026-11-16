use std::path::PathBuf;
use tuxu_core::{execute_archived_plan, ExecuteOptions, NamingRule, Organizer, ScanOptions};

fn main() {
    if let Err(err) = run() {
        eprintln!("archive-once error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(args.next().ok_or("missing data_dir")?);
    let src = PathBuf::from(args.next().ok_or("missing source_root")?);
    let out = PathBuf::from(args.next().ok_or("missing output_root")?);
    let task_name = args.next().ok_or("missing task_name")?;
    let per_group: u32 = args
        .next()
        .ok_or("missing per_group")?
        .parse()
        .map_err(|_| "invalid per_group")?;

    let mut org = Organizer::open(&data).map_err(|e| e.to_string())?;
    org.scan_folder(&src, ScanOptions { recursive: false }, true)
        .map_err(|e| e.to_string())?;
    org.set_task_name(&task_name).map_err(|e| e.to_string())?;
    org.set_output_root(&out).map_err(|e| e.to_string())?;
    org.set_naming_rule(NamingRule {
        preset: "group_seq".into(),
        ..NamingRule::default()
    })
    .map_err(|e| e.to_string())?;
    org.group_by_fixed_size(per_group.max(1), "分组")
        .map_err(|e| e.to_string())?;
    let plan = org.build_plan().map_err(|e| e.to_string())?;
    let report =
        execute_archived_plan(&mut org, &plan.id, ExecuteOptions::default()).map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string(&report).unwrap());
    Ok(())
}
