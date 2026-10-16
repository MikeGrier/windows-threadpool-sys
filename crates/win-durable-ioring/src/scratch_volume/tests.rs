// Copyright (c) 2026 Mike Grier

use std::path::Path;

use super::{
    associated_disk, create_disk_script, create_script, detach_script, free_letter,
    letter_is_taken, online_script, scratch_directory,
};

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
    assert!(!letter_is_taken(letter));
}

#[test]
fn the_system_drive_is_taken_and_a_free_letter_is_not() {
    let system = std::env::var("SystemDrive").expect("Windows names its drive");
    let letter = system.chars().next().expect("a drive letter");
    assert!(letter_is_taken(letter));
    assert!(letter_is_taken(letter.to_ascii_lowercase()));
    let free = free_letter().expect("some letter is free");
    assert_ne!(free.to_ascii_uppercase(), letter.to_ascii_uppercase());
}

#[test]
fn the_scratch_directory_is_a_plain_path_that_names_itself_in_full() {
    let directory = scratch_directory();
    let text = directory.display().to_string();
    assert!(!text.starts_with(r"\\?\"), "{text}");
    assert!(directory.is_dir(), "{text}");
    let canonical = std::fs::canonicalize(&directory).expect("it exists");
    assert_eq!(
        canonical.display().to_string().trim_start_matches(r"\\?\"),
        text,
        "the name is already the long form, so naming it in full changes nothing"
    );
}

#[test]
fn the_disk_script_attaches_with_no_partition_and_prints_what_it_made() {
    let script = create_disk_script(Path::new(r"C:\Temp\d.vhdx"), 64);
    assert_eq!(
        script.lines().collect::<Vec<_>>(),
        [
            r#"create vdisk file="C:\Temp\d.vhdx" maximum=64 type=expandable"#,
            r#"select vdisk file="C:\Temp\d.vhdx""#,
            "attach vdisk",
            "detail vdisk",
        ]
    );
    assert!(!script.contains("partition"), "no partition is made");
    assert!(!script.contains("assign"), "no letter is assigned");
}

#[test]
fn the_online_script_names_the_disk_it_selects() {
    assert_eq!(
        online_script(7).lines().collect::<Vec<_>>(),
        [
            "select disk 7",
            "online disk noerr",
            "attributes disk clear readonly noerr"
        ]
    );
}

/// What `detail vdisk` prints, with the file and the disk it was attached as.
fn detail(filename: &str, disk: &str) -> String {
    format!(
        "\nMicrosoft DiskPart version 10.0.26100.1\n\n\
         Microsoft Virtual Disk\n\
         Device type ID: 3 (VHDX)\n\
         State: Added\n\
         Virtual size:   64 MB\n\
         Physical size: 4096 KB\n\
         Filename: {filename}\n\
         Is Child: No\n\
         Parent Filename: \n\
         Associated disk#: {disk}\n"
    )
}

const VHD: &str = r"C:\Users\someone\AppData\Local\Temp\d.vhdx";

#[test]
fn a_disk_is_read_from_the_output_that_names_the_file_made() {
    let vhd = Path::new(VHD);
    assert_eq!(associated_disk(&detail(VHD, "2"), vhd), Ok(2));
    assert_eq!(associated_disk(&detail(VHD, "  13 "), vhd), Ok(13));
    assert_eq!(
        associated_disk(&detail(&VHD.to_uppercase(), "5"), vhd),
        Ok(5),
        "the name is compared without regard to case, as the file system does"
    );
}

#[test]
fn output_about_some_other_file_is_refused() {
    let other = r"C:\Users\someone\AppData\Local\Temp\other.vhdx";
    let error = associated_disk(&detail(other, "2"), Path::new(VHD)).expect_err("another file");
    assert!(error.contains("not the disk made"), "{error}");
    // A short form of the same directory is not trusted to be the same file either.
    let short = r"C:\Users\SOMEON~1\AppData\Local\Temp\d.vhdx";
    assert!(associated_disk(&detail(short, "2"), Path::new(VHD)).is_err());
}

#[test]
fn disk_zero_is_never_a_scratch_disk() {
    let error = associated_disk(&detail(VHD, "0"), Path::new(VHD)).expect_err("disk 0");
    assert!(error.contains("disk 0"), "{error}");
}

#[test]
fn output_without_exactly_one_file_and_one_disk_is_refused() {
    let vhd = Path::new(VHD);
    for (what, printed) in [
        ("nothing", String::new()),
        ("no number", format!("Filename: {VHD}\n")),
        ("no file", "Associated disk#: 2\n".to_owned()),
        (
            "two disks",
            format!("Filename: {VHD}\nAssociated disk#: 2\nAssociated disk#: 3\n"),
        ),
        (
            "two files",
            format!("Filename: {VHD}\nFilename: {VHD}\nAssociated disk#: 2\n"),
        ),
        ("not a number", detail(VHD, "two")),
        ("negative", detail(VHD, "-1")),
        ("empty", detail(VHD, "")),
        ("too large", detail(VHD, "99999999999")),
    ] {
        assert!(associated_disk(&printed, vhd).is_err(), "{what}");
    }
}
