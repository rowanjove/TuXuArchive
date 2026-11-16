use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tuxu_core::{execute_archived_plan, ExecuteOptions, NamingRule, Organizer, ScanOptions};

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("tuxu-it-{name}-{}-{nanos}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_bmp(path: &Path, w: u32, h: u32) {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        w,
        h,
        image::Rgb([60, 130, 220]),
    ));
    img.write_to(&mut fs::File::create(path).unwrap(), image::ImageFormat::Bmp)
        .unwrap();
}

fn write_jpeg_with_exif(path: &Path, dt: &str) {
    let mut dt_bytes = dt.as_bytes().to_vec();
    dt_bytes.push(0);
    let data_start: u32 = 26 + 2 + 12 + 4;
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&42u16.to_le_bytes());
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x8769u16.to_le_bytes());
    tiff.extend_from_slice(&4u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&26u32.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x9003u16.to_le_bytes());
    tiff.extend_from_slice(&2u16.to_le_bytes());
    tiff.extend_from_slice(&20u32.to_le_bytes());
    tiff.extend_from_slice(&data_start.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(&dt_bytes);

    let mut app1 = Vec::new();
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);
    let len = (app1.len() + 2) as u16;
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&len.to_be_bytes());
    jpeg.extend_from_slice(&app1);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    fs::write(path, jpeg).unwrap();
}

#[test]
fn end_to_end_multi_photo_category_archive_and_safe_undo() {
    let root = temp_dir("e2e");
    let data = root.join("data");
    let src = root.join("src_photos");
    let out = root.join("archive_out");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&out).unwrap();

    // 3 photos on day 1, 2 photos on day 2
    write_jpeg_with_exif(&src.join("IMG_001.jpg"), "2026:05:01 09:00:00");
    write_jpeg_with_exif(&src.join("IMG_002.jpg"), "2026:05:01 09:05:00");
    write_jpeg_with_exif(&src.join("IMG_003.jpg"), "2026:05:01 09:10:00");
    write_jpeg_with_exif(&src.join("IMG_004.jpg"), "2026:05:02 10:00:00");
    write_jpeg_with_exif(&src.join("IMG_005.jpg"), "2026:05:02 10:05:00");

    let mut org = Organizer::open(&data).unwrap();
    let detail = org.scan_folder(&src, ScanOptions { recursive: false }, true).unwrap();
    assert_eq!(detail.counts.scanned, 5);

    org.set_task_name("五一相册").unwrap();
    org.set_output_root(&out).unwrap();
    org.set_dir_layout("task_and_group", "{task}/{group}").unwrap();
    org.set_naming_rule(NamingRule {
        preset: "date_group_seq".into(),
        prefix: "TRIP_".into(),
        date_format: "YYYYMMDD".into(),
        seq_scope: "per_group".into(),
        seq_digits: 3,
        ..NamingRule::default()
    })
    .unwrap();

    // Group by day -> 2 groups, multiple photos per group!
    let d2 = org.group_by_time("day").unwrap();
    assert_eq!(d2.groups.len(), 2);
    assert_eq!(d2.groups[0].count, 3);
    assert_eq!(d2.groups[1].count, 2);

    let plan = org.build_plan().unwrap();
    assert!(!plan.blocked, "plan issues: {:?}", plan.issues);
    assert_eq!(plan.copy_count, 5);

    let report = execute_archived_plan(&mut org, &plan.id, ExecuteOptions::default()).unwrap();
    assert_eq!(report.status, "success");
    assert_eq!(report.succeeded, 5);

    // Verify source untouched
    for i in 1..=5 {
        assert!(src.join(format!("IMG_{i:03}.jpg")).is_file());
    }

    // Verify expected archived files
    let day1 = out.join("五一相册").join("2026-05-01");
    let day2 = out.join("五一相册").join("2026-05-02");
    assert!(day1.join("TRIP_20260501_2026-05-01_001.jpg").is_file());
    assert!(day1.join("TRIP_20260501_2026-05-01_002.jpg").is_file());
    assert!(day1.join("TRIP_20260501_2026-05-01_003.jpg").is_file());
    assert!(day2.join("TRIP_20260502_2026-05-02_001.jpg").is_file());
    assert!(day2.join("TRIP_20260502_2026-05-02_002.jpg").is_file());

    // Tamper with one archived file -> undo must refuse to delete it and preserve it!
    let tampered = day1.join("TRIP_20260501_2026-05-01_002.jpg");
    fs::write(&tampered, b"user-edited-bytes").unwrap();

    let preview = org.undo_preview(&report.job_id).unwrap();
    assert_eq!(preview.will_delete.len(), 4);
    assert_eq!(preview.will_refuse.len(), 1);

    let undo_rep = org.undo_job(&report.job_id).unwrap();
    assert_eq!(undo_rep.deleted.len(), 4);
    assert_eq!(undo_rep.refused.len(), 1);
    assert!(tampered.is_file());
    assert_eq!(fs::read(&tampered).unwrap(), b"user-edited-bytes");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn blocks_duplicate_target_paths_when_sequence_disabled_and_protects_existing() {
    let root = temp_dir("dup-protect");
    let data = root.join("data");
    let src = root.join("src");
    let out = root.join("out");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&out).unwrap();

    write_bmp(&src.join("a.bmp"), 20, 20);
    write_bmp(&src.join("b.bmp"), 20, 20);

    let mut org = Organizer::open(&data).unwrap();
    org.scan_folder(&src, ScanOptions { recursive: false }, true).unwrap();
    org.set_task_name("测试任务").unwrap();
    org.set_output_root(&out).unwrap();
    org.group_by_fixed_size(2, "同一分类").unwrap();

    // Disable sequence numbers with custom pattern {group} -> two files map to same target -> E002 block
    org.set_naming_rule(NamingRule {
        preset: "custom".into(),
        pattern: "{group}".into(),
        seq_scope: "none".into(),
        ..NamingRule::default()
    })
    .unwrap();
    let blocked_plan = org.build_plan().unwrap();
    assert!(blocked_plan.blocked);
    assert!(blocked_plan.issues.iter().any(|i| i.code == "E002"));

    // Now enable sequence -> distinct targets, but pre-create one existing file -> E002 no-overwrite
    org.set_naming_rule(NamingRule {
        preset: "group_seq".into(),
        seq_scope: "per_group".into(),
        seq_digits: 3,
        ..NamingRule::default()
    })
    .unwrap();
    let existing_dir = out.join("测试任务").join("同一分类01");
    fs::create_dir_all(&existing_dir).unwrap();
    let existing_file = existing_dir.join("同一分类01_001.bmp");
    fs::write(&existing_file, b"keep-me").unwrap();

    let plan2 = org.build_plan().unwrap();
    assert!(plan2.blocked);
    assert!(plan2
        .issues
        .iter()
        .any(|i| i.code == "E002" && i.message.contains("目标已存在")));
    assert_eq!(fs::read(&existing_file).unwrap(), b"keep-me");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn handles_arbitrary_manual_selection_and_interrupted_recovery() {
    let root = temp_dir("manual-recover");
    let data = root.join("data");
    let src = root.join("src");
    let out = root.join("out");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&out).unwrap();

    write_bmp(&src.join("01.bmp"), 24, 24);
    write_bmp(&src.join("02.bmp"), 24, 24);
    write_bmp(&src.join("03.bmp"), 24, 24);

    let mut org = Organizer::open(&data).unwrap();
    org.scan_folder(&src, ScanOptions { recursive: false }, true).unwrap();
    org.set_task_name("手动归类").unwrap();
    org.set_output_root(&out).unwrap();
    org.set_dir_layout("group_only", "{group}").unwrap();
    org.set_naming_rule(NamingRule {
        preset: "group_seq".into(),
        seq_digits: 2,
        ..NamingRule::default()
    })
    .unwrap();

    let page = org.photos(None, 0, 10).unwrap();
    // Non-contiguous manual selection: 01 and 03 into "精选", 02 into "备用"
    org.create_group_for_photos(
        "精选",
        &[page.items[0].id.clone(), page.items[2].id.clone()],
    )
    .unwrap();
    org.create_group_for_photos("备用", &[page.items[1].id.clone()])
        .unwrap();

    let plan = org.build_plan().unwrap();
    assert!(!plan.blocked);

    // Simulate interruption during temp write of item 1
    let interrupted = execute_archived_plan(
        &mut org,
        &plan.id,
        ExecuteOptions {
            stop_after_temp_index: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(interrupted.status, "interrupted");
    assert_eq!(org.recovery().unwrap().len(), 1);

    // Retry recovers cleanly
    let resumed = org.retry_job(&interrupted.job_id).unwrap();
    assert_eq!(resumed.status, "success");
    assert_eq!(resumed.succeeded, 3);
    assert!(org.recovery().unwrap().is_empty());
    assert!(out.join("精选").join("精选_01.bmp").is_file());
    assert!(out.join("精选").join("精选_02.bmp").is_file());
    assert!(out.join("备用").join("备用_01.bmp").is_file());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rejects_overlapping_paths_and_handles_1000_photos_performance() {
    let root = temp_dir("perf-1000");
    let data = root.join("data");
    let src = root.join("src");
    let out = root.join("out");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&out).unwrap();

    // Verify overlap rejection
    let org_check = Organizer::open(&data).unwrap();
    let check = org_check.check_paths(
        &src.display().to_string(),
        &src.join("nested").display().to_string(),
    );
    assert!(!check.valid);
    assert!(check.conflict);

    // Create 1,000 tiny valid JPEGs to verify scan + group + plan performance
    for i in 0..1000 {
        let day = (i / 100) + 1;
        let dt = format!("2026:05:{day:02} 10:{:02}:{:02}", (i % 60), (i % 60));
        write_jpeg_with_exif(&src.join(format!("P_{i:04}.jpg")), &dt);
    }

    let t0 = Instant::now();
    let mut org = Organizer::open(&data).unwrap();
    let detail = org.scan_folder(&src, ScanOptions { recursive: false }, true).unwrap();
    assert_eq!(detail.counts.scanned, 1000);
    org.set_task_name("千图性能测试").unwrap();
    org.set_output_root(&out).unwrap();
    org.group_by_time("day").unwrap();
    let plan = org.build_plan().unwrap();
    assert!(!plan.blocked);
    assert_eq!(plan.copy_count, 1000);
    let elapsed = t0.elapsed();
    assert!(
        elapsed.as_secs_f64() < 15.0,
        "1000-photo scan+group+plan took too long: {elapsed:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn flat_and_task_only_layouts_auto_confirm_healthy_photos() {
    let root = temp_dir("flat-layout");
    let data = root.join("data");
    let src = root.join("src");
    let out = root.join("out");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&out).unwrap();

    write_jpeg_with_exif(&src.join("A.jpg"), "2026:05:01 09:00:00");
    write_jpeg_with_exif(&src.join("B.jpg"), "2026:05:01 09:05:00");

    let mut org = Organizer::open(&data).unwrap();
    org.scan_folder(&src, ScanOptions { recursive: false }, true)
        .unwrap();
    org.set_output_root(&out).unwrap();
    let detail = org.set_dir_layout("flat", "").unwrap();
    assert!(!detail.dir_needs_group);
    assert_eq!(detail.counts.ready, 2);

    org.set_naming_rule(NamingRule {
        preset: "prefix_seq".into(),
        prefix: "图_".into(),
        ..NamingRule::default()
    })
    .unwrap();
    let plan = org.build_plan().unwrap();
    assert!(!plan.blocked);
    assert_eq!(plan.copy_count, 2);

    let rep = execute_archived_plan(&mut org, &plan.id, ExecuteOptions::default()).unwrap();
    assert_eq!(rep.status, "success");
    assert!(out.join("图_001.jpg").is_file());
    assert!(out.join("图_002.jpg").is_file());

    let _ = fs::remove_dir_all(&root);
}
