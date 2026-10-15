// Copyright (c) 2026 Mike Grier

use std::path::Path;

use super::{create_script, detach_script, free_letter};

#[test]
fn the_create_script_makes_attaches_partitions_and_assigns() {
    let script = create_script(Path::new(r"C:\Temp\s.vhdx"), 64, 'Y');
    let lines: Vec<&str> = script.lines().collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(
        lines[0],
        r#"create vdisk file="C:\Temp\s.vhdx" maximum=64 type=expandable"#
    );
    assert_eq!(lines[1], r#"select vdisk file="C:\Temp\s.vhdx""#);
    assert_eq!(lines[2], "attach vdisk");
    assert_eq!(lines[3], "create partition primary");
    assert_eq!(lines[4], "assign letter=Y");
}

#[test]
fn the_detach_script_selects_the_disk_it_detaches() {
    let script = detach_script(Path::new(r"C:\Temp\s.vhdx"));
    assert_eq!(
        script.lines().collect::<Vec<_>>(),
        [r#"select vdisk file="C:\Temp\s.vhdx""#, "detach vdisk"]
    );
}

#[test]
fn a_free_letter_is_one_nothing_is_mounted_at() {
    let letter = free_letter().expect("some letter is free");
    assert!(letter.is_ascii_uppercase());
    assert!(!Path::new(&format!("{letter}:\\")).exists());
}
