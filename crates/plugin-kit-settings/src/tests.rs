#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn the_shared_root_follows_each_system_and_the_explicit_override() {
    let variables = [
        ("HOME", "user"),
        ("APPDATA", "roaming"),
        ("XDG_DATA_HOME", "data"),
    ];
    let get = |key: &str| {
        variables
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).into())
    };
    assert_eq!(
        root_for("windows", get),
        PathBuf::from("roaming/Idle Foundry")
    );
    assert_eq!(
        root_for("macos", get),
        PathBuf::from("user/Library/Application Support/Idle Foundry")
    );
    assert_eq!(root_for("linux", get), PathBuf::from("data/Idle Foundry"));
    assert_eq!(
        root_for("linux", |k| (k == "HOME").then(|| "user".into())),
        PathBuf::from("user/.local/share/Idle Foundry")
    );
    assert_eq!(
        root_for("linux", |_| None),
        PathBuf::from("idle-foundry-shared")
    );
    for system in ["windows", "macos", "linux"] {
        assert_eq!(
            root_for(system, |k| (k == "IDLE_FOUNDRY_SHARED")
                .then(|| "shared".into())),
            PathBuf::from("shared")
        );
    }
    for name in [
        "", ".", "..", "../Other", "x/y", "x\\y", "x:", " x", "x.", "CON", "Nul.txt", "LPT³",
    ] {
        assert!(plugin_dir(name).is_err(), "{name}");
    }
}

/// The CA-72's R51 test, without its skin or other instrument-specific choices.
#[test]
fn the_settings_are_kept_and_a_bad_file_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings::at(dir.path().join("Instrument/settings.toml"));
    assert!(settings.read().unwrap().is_none());
    settings
        .change(|t| {
            t.insert("indicator", value(false));
            Ok(())
        })
        .unwrap();
    assert_eq!(
        settings.read().unwrap().unwrap()["indicator"].as_bool(),
        Some(false)
    );
    {
        let text = "not [ toml";
        std::fs::write(settings.path(), text).unwrap();
        assert!(settings.read().is_err());
        assert!(
            settings
                .change(|t| {
                    t.insert("indicator", value(true));
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(settings.path()).unwrap(), text);
    }
    std::fs::write(settings.path(), [255, 254]).unwrap();
    assert!(settings.change(|_| Ok(())).is_err());
    assert_eq!(std::fs::read(settings.path()).unwrap(), [255, 254]);
}

/// Per-key changes keep the other editor's latest choices and unknown fields,
/// including nested fields (the CA-72's and CA-74's later settings review).
#[test]
fn one_editors_choice_does_not_undo_anothers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "future = 'kept'\n[extra]\nvalue = 3\n").unwrap();
    let a = Settings::at(&path);
    let b = Settings::at(&path);
    assert!(a.read().unwrap().unwrap().get("indicator").is_none());
    assert!(b.read().unwrap().unwrap().get("note_read").is_none());
    a.change(|t| {
        t.insert("indicator", value(false));
        Ok(())
    })
    .unwrap();
    b.change(|t| {
        t.insert("note_read", value(true));
        Ok(())
    })
    .unwrap();
    let table = a.read().unwrap().unwrap();
    assert_eq!(table["indicator"].as_bool(), Some(false));
    assert_eq!(table["note_read"].as_bool(), Some(true));
    assert_eq!(table["future"].as_str(), Some("kept"));
    assert_eq!(table["extra"]["value"].as_integer(), Some(3));
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        a.change(|t| {
            t.insert("indicator", value(true));
            Err("Abandon the change".into())
        })
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn simultaneous_settings_changes_keep_every_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "future = 'kept'\n").unwrap();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for n in 0..8 {
            let (path, barrier) = (&path, &barrier);
            scope.spawn(move || {
                barrier.wait();
                Settings::at(path)
                    .change(|t| {
                        t.insert(&format!("choice{n}"), value(n));
                        Ok(())
                    })
                    .unwrap();
            });
        }
    });
    let table = Settings::at(path).read().unwrap().unwrap();
    for n in 0..8 {
        assert_eq!(table[&format!("choice{n}")].as_integer(), Some(n));
    }
    assert_eq!(table["future"].as_str(), Some("kept"));
}

#[test]
fn complete_writes_use_unique_temporary_files_and_preserve_the_original_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let collision = path.with_extension("toml.tmp");
    std::fs::write(&collision, "another writer").unwrap();
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for n in 0..4 {
            let (path, barrier) = (&path, &barrier);
            scope.spawn(move || {
                barrier.wait();
                Settings::at(path)
                    .change(|t| {
                        t.insert("text", value(format!("{n}").repeat(4096)));
                        Ok(())
                    })
                    .unwrap();
            });
        }
    });
    let table = Settings::at(&path).read().unwrap().unwrap();
    let text = table["text"].as_str().unwrap();
    assert_eq!(text.len(), 4096);
    assert!(text.bytes().all(|b| b == text.as_bytes()[0]));
    assert_eq!(
        std::fs::read_to_string(collision).unwrap(),
        "another writer"
    );
    let blocked = dir.path().join("blocked.toml");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("kept"), "original").unwrap();
    assert!(write_atomic(&blocked, "replacement").is_err());
    assert_eq!(
        std::fs::read_to_string(blocked.join("kept")).unwrap(),
        "original"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 4);
}

#[test]
fn unknown_toml_values_survive_a_settings_change() {
    let text = r#"
"a.b" = "Quote \" slash \\ newline\n tab\t back\b form\f nul\u0000 café 🎵"
integer = 42
float = 42.0
negative_zero = -0.0
infinity = inf
negative_infinity = -inf
nan = nan
offset = 1979-05-27T07:32:00-08:00
local = 1979-05-27T07:32:00
date = 1979-05-27
time = 07:32:00
mixed = [1, "two", { three = true }]
inline = { nested = { flag = true }, values = [1, 2] }
[nested."a.b"]
flag = true
[[future]]
name = 'first'
[future.nested]
number = 1
[[future]]
name = 'second'
[future.nested]
number = 2
"#;
    let table = parse(text).unwrap();
    let encoded = encode(&table).unwrap();
    let reread = parse(&encoded).unwrap();
    assert_eq!(reread["a.b"].as_str(), table["a.b"].as_str());
    assert_eq!(reread["float"].as_float(), Some(42.0));
    assert_eq!(
        reread["negative_zero"].as_float().unwrap().to_bits(),
        (-0.0f64).to_bits()
    );
    assert!(reread["nan"].as_float().unwrap().is_nan());
    assert_eq!(reread["inline"]["nested"]["flag"].as_bool(), Some(true));
    let future = reread["future"].as_array_of_tables().unwrap();
    assert_eq!(future.len(), 2);
    assert_eq!(
        future.get(1).unwrap()["nested"]["number"].as_integer(),
        Some(2)
    );
    assert_eq!(encode(&reread).unwrap(), encoded);
}

#[test]
fn preserving_a_section_does_not_lose_its_earlier_dotted_assignments() {
    let source = "name = 'Own'\nextra.earlier = 42\n[extra.nested]\nvalue = 'kept'\n";
    let mut table = parse(source).unwrap();
    table.insert("name", value("Renamed"));
    let text = encode_preserving(&table, source).unwrap();
    let result = parse(&text).unwrap();
    assert_eq!(result["extra"]["earlier"].as_integer(), Some(42));
    assert_eq!(result["extra"]["nested"]["value"].as_str(), Some("kept"));
}

#[test]
fn future_settings_are_read_and_rewritten_with_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings::at(dir.path().join("settings.toml"));
    for format in ["99", "'future'"] {
        std::fs::write(
            settings.path(),
            format!("format = {format}\nunknown = 'kept'\n"),
        )
        .unwrap();
        assert_eq!(
            settings.read().unwrap().unwrap()["unknown"].as_str(),
            Some("kept")
        );
        settings
            .change(|t| {
                t.insert("width", value(1200));
                Ok(())
            })
            .unwrap();
        let t = settings.read().unwrap().unwrap();
        assert_eq!(t["format"].as_integer(), Some(1));
        assert_eq!(t["unknown"].as_str(), Some("kept"));
    }
}

#[cfg(unix)]
#[test]
fn replacements_use_normal_creation_permissions_and_write_only_locks_work() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let normal = dir.path().join("normal");
    let file = dir.path().join("settings.toml");
    std::fs::write(&normal, "").unwrap();
    write_atomic(&file, "first = true\n").unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&file), mode(&normal));
    let lockfile = dir.path().join("settings.toml.lock");
    std::fs::write(&lockfile, "").unwrap();
    std::fs::set_permissions(&lockfile, std::fs::Permissions::from_mode(0o200)).unwrap();
    drop(lock(&lockfile).unwrap());
}
