//! Storage regressions from the CA-72's R10 and R18, with a small general factory
//! fixture instead of its instrument's sounds. Parameter-specific migrations,
//! gain curves, packs and hosted effects remain the plug-in's codec tests.

#![allow(clippy::unwrap_used)]
use super::*;

const FACTORY: &str = r#"
format = 1
[[preset]]
name = "Felt Bass"
description = "A soft sound."
tags = ["bass"]
[preset.values]
cutoff = 1.5
[[preset]]
name = "Singing Drift"
description = "A sustained sound."
tags = ["lead", "glide", "prog"]
[preset.values]
cutoff = -2.5
[[preset]]
name = "Deep Boom"
tags = ["low"]
[preset.values]
cutoff = 0.5
"#;

fn library(dir: &Path) -> Library<PlainValues> {
    Library::at(dir, FACTORY, PlainValues::default()).unwrap()
}
fn factory() -> Vec<Sound<Vec<(String, f64)>>> {
    parse_factory(FACTORY, &PlainValues::default()).unwrap()
}
fn names(list: &[Entry<Vec<(String, f64)>>]) -> Vec<&str> {
    list.iter().map(|e| e.sound.name.as_str()).collect()
}
fn toml_files(dir: &Path) -> Vec<String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".toml"))
        .collect();
    files.sort();
    files
}

#[test]
fn the_users_presets_are_saved_found_tagged_renamed_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    let values = vec![("cutoff".to_owned(), -2.5), ("osc1_range".to_owned(), 3.0)];
    let e = lib
        .save(
            "Deep Bass",
            values,
            Some(parse_tags("bass, midnight, Bass").unwrap()),
        )
        .unwrap();
    assert_eq!(e.origin, Origin::User);
    assert_eq!(e.sound.tags, ["bass", "midnight"]);
    // The file as the DAW reads it.
    let t = parse(&(std::fs::read_to_string(dir.path().join("Deep Bass.toml")).unwrap())).unwrap();
    assert_eq!(t["format"].as_integer(), Some(1));
    assert_eq!(t["values"]["cutoff"].as_float(), Some(-2.5));
    assert_eq!(t["values"]["osc1_range"].as_integer(), Some(3));
    assert_eq!(
        names(&lib.search("ROUND deep", &[], false, false)),
        Vec::<&str>::new()
    );
    // (Deep Boom, a factory preset since 0.3.0.)
    assert_eq!(
        names(&lib.search("deep", &[], false, false)),
        ["Deep Bass", "Deep Boom"]
    );
    assert_eq!(
        names(&lib.search("", &["midnight".into()], false, false)),
        ["Deep Bass"]
    );
    assert_eq!(names(&lib.search("", &[], false, true)), ["Deep Bass"]);
    lib.favorite("deep bass", true).unwrap();
    lib.favorite("Singing Drift", true).unwrap();
    assert_eq!(
        names(&lib.search("", &[], true, false)),
        ["Deep Bass", "Singing Drift"]
    );
    lib.tag("Felt Bass", vec!["bass".into(), "mine".into()])
        .unwrap();
    assert_eq!(
        names(&lib.search("", &["mine".into()], false, false)),
        ["Felt Bass"]
    );
    assert!(lib.tags().contains(&"midnight".to_owned()));
    lib.rename("Deep Bass", "Deeper Bass").unwrap();
    assert!(!dir.path().join("Deep Bass.toml").exists());
    assert!(lib.find("Deeper Bass").unwrap().favorite);
    assert!(lib.rename("Deeper Bass", "singing drift").is_err());
    lib.remove("Deeper Bass").unwrap();
    assert!(lib.search("", &[], false, true).is_empty());
    assert!(check_name("a/b").is_err() && check_name(" x").is_err() && check_name("").is_err());
}

#[test]
fn a_factory_preset_edited_renamed_or_removed_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    let edited = lib
        .save("singing drift", vec![("emphasis".into(), 9.0)], None)
        .unwrap();
    assert_eq!(edited.origin, Origin::Edited);
    assert_eq!(edited.replaces.as_deref(), Some("Singing Drift"));
    assert_eq!(edited.sound.tags, ["lead", "glide", "prog"]);
    assert_eq!(
        lib.entries()
            .0
            .iter()
            .filter(|e| same(&e.sound.name, "singing drift"))
            .count(),
        1
    );
    assert_eq!(lib.revert("singing drift").unwrap(), "Singing Drift");
    assert_eq!(lib.find("Singing Drift").unwrap().origin, Origin::Factory);
    lib.rename("Felt Bass", "Round Bass").unwrap();
    assert!(lib.find("Felt Bass").is_none());
    assert_eq!(lib.find("Round Bass").unwrap().origin, Origin::Edited);
    lib.revert("Round Bass").unwrap();
    assert!(lib.find("Felt Bass").is_some());
    lib.remove("Felt Bass").unwrap();
    lib.save("Singing Drift", vec![], None).unwrap();
    lib.remove("Singing Drift").unwrap();
    assert!(lib.find("Felt Bass").is_none() && lib.find("Singing Drift").is_none());
    assert!(!dir.path().join("Singing Drift.toml").exists());
    assert_eq!(lib.restore().unwrap(), ["Felt Bass", "Singing Drift"]);
    assert_eq!(lib.entries().0.len(), factory().len());
    assert!(lib.revert("Felt Bass").is_err());
}

#[test]
fn factory_is_no_presets_name() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    lib.favorite("Singing Drift", true).unwrap();
    lib.tag("Felt Bass", vec!["mine".into()]).unwrap();
    for n in ["factory", "Factory", "FACTORY"] {
        assert!(
            lib.save(n, vec![("cutoff".into(), 1.0)], None).is_err(),
            "{n}"
        );
        assert!(lib.rename("Singing Drift", n).is_err(), "{n}");
    }
    assert!(lib.find("Singing Drift").unwrap().favorite);
    assert_eq!(lib.find("Felt Bass").unwrap().sound.tags, ["mine"]);
    assert!(lib.save("Factory Floor", vec![], None).is_ok());
}

#[test]
fn windows_device_names_are_refused_everywhere() {
    for n in [
        "CON",
        "con",
        "Prn",
        "AUX",
        "NUL",
        "nul.txt",
        "NUL.tar.gz",
        "COM1",
        "com9",
        "COM0",
        "LPT1",
        "lpt9.x",
        "COM¹",
        "LPT³",
        "CONIN$",
        "conout$",
        "Aux .x",
        "Dot.",
        "Space ",
        " Space",
        ".Dot",
    ] {
        assert!(check_name(n).is_err(), "{n:?}");
    }
    for n in [
        "CONSOLE",
        "Nullify",
        "COM10",
        "LPT",
        "Com Bass",
        "Auxiliary Lead",
        "a.con",
        "Bass 2",
    ] {
        assert!(check_name(n).is_ok(), "{n:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    assert!(lib.save("aux", vec![], None).is_err());
    assert!(lib.rename("Singing Drift", "NUL.lead").is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_files_unsafe_name_is_reported_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    for (file, name) in [
        ("up.toml", "../../outside"),
        ("root.toml", "/tmp/outside"),
        ("device.toml", "NUL"),
        ("own.toml", "factory"),
        ("fine.toml", "Fine"),
    ] {
        std::fs::write(
            dir.path().join(file),
            format!("format = 1\nname = {name:?}\n\n[values]\ncutoff = 1\n"),
        )
        .unwrap();
    }
    let lib = library(dir.path());
    let (list, bad) = lib.entries();
    let mine: Vec<&str> = list
        .iter()
        .filter(|e| e.origin == Origin::User)
        .map(|e| e.sound.name.as_str())
        .collect();
    assert_eq!(mine, ["Fine"]);
    assert_eq!(bad.len(), 4, "{bad:?}");
}

#[test]
fn an_unreadable_overlay_is_left_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    lib.save("Singing Drift", vec![("cutoff".into(), 1.0)], None)
        .unwrap();
    let overlay = dir.path().join("factory.toml");
    for text in [
        "format = 1\n[presets.\"Felt Bass\"\nfavorite = true\n",
        "format = 2\n[presets.\"Felt Bass\"]\nfavorite = true\nrating = 5\n",
        "format = 1\npresets = 3\n",
    ] {
        std::fs::write(&overlay, text).unwrap();
        let e = lib.favorite("Felt Bass", true).unwrap_err();
        assert!(!e.is_empty());
        assert!(lib.tag("Felt Bass", vec!["x".into()]).is_err());
        assert!(lib.remove("Felt Bass").is_err());
        assert!(lib.remove("Singing Drift").is_err());
        assert!(lib.revert("Singing Drift").is_err());
        assert!(lib.restore().is_err());
        assert_eq!(std::fs::read_to_string(&overlay).unwrap(), text);
        assert!(dir.path().join("Singing Drift.toml").exists());
        let (list, bad) = lib.entries();
        assert_eq!(list.len(), factory().len());
        assert_eq!(bad.len(), 1, "{bad:?}");
    }
}

#[test]
fn the_overlay_keeps_what_it_does_not_know() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    let overlay = dir.path().join("factory.toml");
    std::fs::write(
        &overlay,
        "format = 1\nnote = \"kept\"\n\n[presets.\"Singing Drift\"]\nfavorite = true\nrating = 5\n\n[presets.\"felt bass\"]\nhidden = true\ntags = [\"low\"]\n",
    )
    .unwrap();
    assert!(lib.find("Singing Drift").unwrap().favorite);
    assert!(
        lib.find("Felt Bass").is_none(),
        "hidden whatever the key's case"
    );
    lib.favorite("Singing Drift", false).unwrap();
    assert_eq!(lib.restore().unwrap(), ["felt bass"]);
    assert_eq!(lib.find("Felt Bass").unwrap().sound.tags, ["low"]);
    lib.tag("Felt Bass", vec!["round".into()]).unwrap();
    let t = parse(&(std::fs::read_to_string(&overlay).unwrap())).unwrap();
    assert_eq!(t["note"].as_str(), Some("kept"));
    assert_eq!(
        t["presets"]["Singing Drift"]["rating"].as_integer(),
        Some(5)
    );
    assert!(t["presets"]["Singing Drift"].get("favorite").is_none());
    assert_eq!(t["presets"]["felt bass"]["tags"][0].as_str(), Some("round"));
    assert!(t["presets"].get("Felt Bass").is_none());
    assert_eq!(lib.find("Felt Bass").unwrap().sound.tags, ["round"]);
}

#[test]
fn marks_go_to_the_presets_own_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("my_patch.toml"),
        "format = 1\nname = \"My Patch\"\n\n[values]\ncutoff = 1\n",
    )
    .unwrap();
    let lib = library(dir.path());
    lib.favorite("My Patch", true).unwrap();
    lib.tag("my patch", vec!["round".into()]).unwrap();
    assert_eq!(toml_files(dir.path()), ["my_patch.toml"]);
    let e = lib.find("My Patch").unwrap();
    assert!(e.favorite && e.sound.tags == ["round"]);
    assert_eq!(lib.search("", &[], false, true).len(), 1);
    // Saved again, it takes its name's file (nothing else there), still one.
    lib.save("My Patch", vec![("cutoff".into(), 2.0)], None)
        .unwrap();
    assert_eq!(toml_files(dir.path()), ["My Patch.toml"]);
    assert!(lib.find("My Patch").unwrap().favorite);
}

#[test]
fn a_name_changed_in_case_leaves_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    lib.save(
        "Deep Bass",
        vec![("cutoff".into(), 1.0)],
        Some(vec!["low".into()]),
    )
    .unwrap();
    lib.save("deep bass", vec![("cutoff".into(), 2.0)], None)
        .unwrap();
    assert_eq!(toml_files(dir.path()), ["deep bass.toml"]);
    let e = lib.find("Deep Bass").unwrap();
    assert_eq!(e.sound.name, "deep bass");
    assert_eq!(e.sound.tags, ["low"]);
    assert!(e.sound.parameters.contains(&("cutoff".to_owned(), 2.0)));
    lib.rename("deep bass", "DEEP BASS").unwrap();
    assert_eq!(toml_files(dir.path()), ["DEEP BASS.toml"]);
    assert_eq!(lib.search("", &[], false, true).len(), 1);
}

#[test]
fn a_name_is_one_presets_and_another_file_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let preset = |file: &str, name: &str| {
        std::fs::write(
            dir.path().join(file),
            format!("format = 1\nname = {name:?}\n\n[values]\ncutoff = 1\n"),
        )
        .unwrap();
    };
    preset("a.toml", "Twin");
    preset("b.toml", "twin");
    preset("c.toml", "Third");
    std::fs::write(dir.path().join("Third.toml"), "name = ").unwrap();
    std::fs::write(dir.path().join("Fourth.toml"), "format = 9\n").unwrap();
    let lib = library(dir.path());
    let (list, bad) = lib.entries();
    let twins = list.iter().filter(|e| same(&e.sound.name, "twin")).count();
    assert_eq!(twins, 1);
    assert_eq!(bad.len(), 3, "{bad:?}");
    // Third's own file takes it; the file of its name is not its own and stays.
    lib.save("Third", vec![("cutoff".into(), 3.0)], None)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Third.toml")).unwrap(),
        "name = "
    );
    assert!(
        lib.find("Third")
            .unwrap()
            .sound
            .parameters
            .contains(&("cutoff".to_owned(), 3.0))
    );
    assert!(lib.save("Fourth", vec![], None).is_err());
    assert!(lib.rename("Twin", "fourth").is_ok(), "into its own file");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Fourth.toml")).unwrap(),
        "format = 9\n"
    );
}

#[test]
fn replaces_is_the_factorys_name_in_any_case() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Mine.toml"),
        "format = 1\nname = \"Mine\"\nreplaces = \"singing drift\"\n\n[values]\ncutoff = 1\n",
    )
    .unwrap();
    let lib = library(dir.path());
    let e = lib.find("Mine").unwrap();
    assert_eq!(
        (e.origin, e.replaces.as_deref()),
        (Origin::Edited, Some("Singing Drift"))
    );
    assert!(lib.find("Singing Drift").is_none());
    lib.remove("Mine").unwrap();
    assert!(lib.find("Singing Drift").is_none() && lib.find("Mine").is_none());
    assert_eq!(lib.restore().unwrap(), ["Singing Drift"]);
}

#[test]
fn the_folder_is_read_again_only_when_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    let file = dir.path().join("Pad.toml");
    let text = "format = 1\nname = \"Pad\"\n\n[values]\ncutoff = 1\n";
    std::fs::write(&file, text).unwrap();
    assert!(lib.find("Pad").is_some());
    let then = std::fs::metadata(&file).unwrap().modified().unwrap();
    let touch = |t: SystemTime| {
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(t)
            .unwrap();
    };
    // Garbage of the same size at the same time: not parsed, Pad as read.
    std::fs::write(&file, "#".repeat(text.len())).unwrap();
    touch(then);
    assert!(
        lib.find("Pad").is_some(),
        "parsed again, the folder unchanged"
    );
    assert_eq!(lib.search("pad", &[], false, true).len(), 1);
    // Another time: parsed, and it has no name.
    touch(then + std::time::Duration::from_secs(2));
    assert!(lib.find("Pad").is_none());
    assert_eq!(lib.entries().1.len(), 1);
    // A file more, and the library's own writes.
    lib.save("Lid", vec![], Some(vec!["new".into()])).unwrap();
    assert!(lib.tags().contains(&"new".to_owned()));
    lib.favorite("Lid", true).unwrap();
    assert!(lib.find("Lid").unwrap().favorite);
}

#[test]
fn a_preset_written_by_the_daw_is_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Wide Pad.toml"),
        "format = 1\nname = \"Wide Pad\"\ntags = [\"pad\"]\nfavorite = false\n\n[values]\ncutoff = 1.5\nloudness_attack = 6\npoly = 1\nvoices = 6\nentropy = 40\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("Broken.toml"), "name = ").unwrap();
    let lib = library(dir.path());
    let (list, bad) = lib.entries();
    let pad = list.iter().find(|e| e.sound.name == "Wide Pad").unwrap();
    assert_eq!(pad.origin, Origin::User);
    assert!(pad.sound.parameters.contains(&("voices".to_owned(), 6.0)));
    assert_eq!(bad.len(), 1);
}

#[test]
fn bad_files_are_reported_and_no_save_or_rename_overwrites_them() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    let bad = [
        ("Broken.toml", "name = "),
        ("Future.toml", "format = 9\nname = 'Future'\n"),
        ("Wrong.toml", "format = 'one'\nname = 'Wrong'\n"),
        ("Infinite.toml", "name = 'Infinite'\n[values]\nx = inf\n"),
    ];
    for (file, text) in bad {
        std::fs::write(dir.path().join(file), text).unwrap();
    }
    assert_eq!(lib.entries().1.len(), bad.len());
    for name in ["Broken", "Future", "Wrong", "Infinite"] {
        assert!(lib.save(name, vec![], None).is_err());
        assert!(lib.rename("Felt Bass", name).is_err());
    }
    lib.save("New", vec![("amount".into(), 0.25)], None)
        .unwrap();
    for (file, text) in bad {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(file)).unwrap(),
            text
        );
    }
}

#[test]
fn the_factory_source_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let factory_file = dir.path().join("built-in.toml");
    std::fs::write(&factory_file, FACTORY).unwrap();
    let presets = dir.path().join("Presets");
    let lib = Library::at(
        &presets,
        &std::fs::read_to_string(&factory_file).unwrap(),
        PlainValues::default(),
    )
    .unwrap();
    lib.favorite("Felt Bass", true).unwrap();
    lib.tag("Felt Bass", vec!["mine".into()]).unwrap();
    lib.save("Felt Bass", vec![("cutoff".into(), 9.0)], None)
        .unwrap();
    assert_eq!(lib.find("Felt Bass").unwrap().origin, Origin::Edited);
    lib.rename("Felt Bass", "My Sound").unwrap();
    lib.revert("My Sound").unwrap();
    assert_eq!(
        lib.find("Felt Bass").unwrap().sound.parameters,
        factory()[0].parameters
    );
    lib.remove("Felt Bass").unwrap();
    lib.restore().unwrap();
    assert_eq!(std::fs::read_to_string(factory_file).unwrap(), FACTORY);
    assert_eq!(
        parse_factory(FACTORY, &PlainValues::default()).unwrap()[0].parameters,
        factory()[0].parameters
    );
}

#[test]
fn factory_parse_errors_are_visible_instead_of_an_empty_library() {
    for text in [
        "name = ",
        "format = 2",
        "format = 1\npreset = 3",
        "format = 1\n[[preset]]\nname = 'Twin'\n[[preset]]\nname = 'twin'",
        "format = 1\n[[preset]]\nname = '../Outside'",
        "format = 1\n[[preset]]\nname = 'Bad'\ntags = [3]",
    ] {
        assert!(
            Library::at("unused", text, PlainValues::default()).is_err(),
            "{text}"
        );
    }
}

#[test]
fn player_controls_are_supplied_by_the_caller_and_only_omitted_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let codec = PlainValues {
        kept_on_save: vec!["performance".into()],
    };
    let lib = Library::at(dir.path(), FACTORY, codec.clone()).unwrap();
    let e = lib
        .save(
            "Own",
            vec![("performance".into(), 2.0), ("amount".into(), 3.0)],
            None,
        )
        .unwrap();
    assert_eq!(e.sound.parameters, [("amount".to_owned(), 3.0)]);
    let hand_written = "name = 'Written'\n[values]\nperformance = 4\nswitch = true\n";
    assert_eq!(
        parse_user(hand_written, &codec).unwrap().parameters,
        [("performance".to_owned(), 4.0), ("switch".to_owned(), 1.0)]
    );
    let path = dir.path().join("Written.toml");
    std::fs::write(&path, hand_written).unwrap();
    lib.favorite("Written", true).unwrap();
    assert!(
        lib.find("Written")
            .unwrap()
            .sound
            .parameters
            .contains(&("performance".to_owned(), 4.0))
    );
}

#[derive(Clone, Debug)]
struct Labelled;

impl PresetCodec for Labelled {
    type Parameters = (String, Vec<(String, f64)>);
    fn read(&self, table: &Table, defaults: &Table) -> Result<Self::Parameters, String> {
        let label = table
            .get("law")
            .or_else(|| defaults.get("law"))
            .and_then(Item::as_str)
            .unwrap_or("legacy");
        Ok((label.into(), PlainValues::default().read(table, defaults)?))
    }
    fn write(&self, parameters: &Self::Parameters, table: &mut Table) -> Result<(), String> {
        table.insert("law", value(parameters.0.as_str()));
        table.insert("version", value("current"));
        PlainValues::default().write(&parameters.1, table)
    }
}

#[test]
fn a_plugins_codec_keeps_its_own_types_and_factory_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::at(
        dir.path(),
        &FACTORY.replace("format = 1", "format = 1\nlaw = 'factory'"),
        Labelled,
    )
    .unwrap();
    assert_eq!(lib.find("Felt Bass").unwrap().sound.parameters.0, "factory");
    lib.rename("Felt Bass", "Renamed Factory").unwrap();
    assert_eq!(
        lib.find("Renamed Factory").unwrap().sound.parameters.0,
        "factory"
    );
    assert_eq!(
        parse_user("name = 'Old'", &Labelled).unwrap().parameters.0,
        "legacy"
    );
    let data = (
        "chosen".to_owned(),
        vec![("amount".to_owned(), 0.321_987_654_321)],
    );
    lib.save("New", data.clone(), None).unwrap();
    lib.rename("New", "Renamed").unwrap();
    lib.tag("Renamed", vec!["tag".into()]).unwrap();
    assert_eq!(lib.find("Renamed").unwrap().sound.parameters, data);
    let text = std::fs::read_to_string(dir.path().join("Renamed.toml")).unwrap();
    assert_eq!(parse(&text).unwrap()["version"].as_str(), Some("current"));
}

#[test]
fn a_failed_codec_save_leaves_the_previous_sound_and_filename() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    lib.save("Own", vec![("amount".into(), 1.0)], None).unwrap();
    let file = dir.path().join("Own.toml");
    let original = std::fs::read(&file).unwrap();
    assert!(
        lib.save("own", vec![("amount".into(), f64::NAN)], None)
            .is_err()
    );
    assert_eq!(std::fs::read(file).unwrap(), original);
    assert_eq!(toml_files(dir.path()), ["Own.toml"]);
}

/// The CA-72's R-FX-PRESETS preservation tests, made general: an unchanged
/// section remains opaque even through a save, rename, favourite or tags change.
#[test]
fn opaque_sections_keep_every_byte_including_mixed_line_endings() {
    let template = "\n# untouched\n[extra] # here\nstate = '''{\"format\":1,\"slots\":[]}'''\ntext = '''\n[values]\nnot_a_header = true\n'''\n[[extra.future]]\nvalue = 0xDEAD # as typed\n";
    for raw in [
        template.to_owned(),
        template.replace('\n', "\r\n"),
        template.replace("# here\n", "# here\r\n"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let lib = library(dir.path());
        let file = dir.path().join("Opaque.toml");
        std::fs::write(
            &file,
            format!("name = 'Opaque'\n{raw}\n[values]\namount = 1\n"),
        )
        .unwrap();
        lib.save("Opaque", vec![("amount".into(), 2.0)], None)
            .unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains(&raw));
        lib.rename("Opaque", "Renamed").unwrap();
        lib.favorite("Renamed", true).unwrap();
        lib.tag("Renamed", vec!["tag".into()]).unwrap();
        let text = std::fs::read_to_string(dir.path().join("Renamed.toml")).unwrap();
        assert!(text.contains(&raw), "{text:?}");
        assert_eq!(
            lib.find("Renamed").unwrap().sound.parameters,
            [("amount".to_owned(), 2.0)]
        );
    }
}

#[test]
fn simultaneous_library_changes_keep_both_factory_marks_and_user_files() {
    let dir = tempfile::tempdir().unwrap();
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for name in ["Felt Bass", "Singing Drift", "One", "Two"] {
            let (path, barrier) = (dir.path(), &barrier);
            scope.spawn(move || {
                let lib = library(path);
                barrier.wait();
                if name == "One" || name == "Two" {
                    lib.save(name, vec![], None).unwrap();
                } else {
                    lib.favorite(name, true).unwrap();
                }
            });
        }
    });
    let lib = library(dir.path());
    assert!(lib.find("Felt Bass").unwrap().favorite);
    assert!(lib.find("Singing Drift").unwrap().favorite);
    assert!(lib.find("One").is_some() && lib.find("Two").is_some());
    assert!(lib.entries().1.is_empty());
}

#[test]
fn unchanged_factory_marks_do_not_create_or_rewrite_an_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path());
    assert!(lib.restore().unwrap().is_empty());
    lib.favorite("Felt Bass", false).unwrap();
    let path = dir.path().join("factory.toml");
    assert!(!path.exists());
    lib.favorite("Felt Bass", true).unwrap();
    let before = std::fs::read(&path).unwrap();
    let time = std::fs::metadata(&path).unwrap().modified().unwrap();
    lib.favorite("Felt Bass", true).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), time);
}
