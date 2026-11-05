use super::*;

fn preset(name: &str, origin: Origin, favorite: bool, tags: &[&str]) -> Preset {
    Preset {
        name: name.into(),
        description: String::new(),
        tags: tags.iter().map(|t| (*t).into()).collect(),
        favorite,
        origin,
    }
}

fn entries() -> Vec<Preset> {
    vec![
        preset("Pad A", Origin::User, true, &["soft", "wide"]),
        preset("Pad B", Origin::User, false, &["soft"]),
        preset("Factory Pad", Origin::Factory, true, &["wide"]),
        preset("Edited Bass", Origin::Edited, false, &["bass"]),
    ]
}

fn browser() -> BrowserState {
    let mut b = BrowserState::new(2);
    b.refresh(entries());
    b.show(true);
    b
}

fn names(b: &BrowserState) -> Vec<&str> {
    b.list().iter().map(|e| e.name.as_str()).collect()
}
fn request(effects: Vec<BrowserEffect>) -> Request {
    effects
        .into_iter()
        .find_map(|e| match e {
            BrowserEffect::Request(r) => Some(r),
            _ => None,
        })
        .expect("an operation")
}
fn replace(b: &mut BrowserState, text: &str) {
    b.key(BrowserKey::Text(TextEdit::SelectAll));
    b.key(BrowserKey::Text(TextEdit::Insert(text)));
}

#[test]
fn search_tags_and_tabs_work_together_on_the_snapshot() {
    let mut b = browser();
    b.set_search("pAd");
    assert_eq!(names(&b), ["Factory Pad", "Pad A", "Pad B"]);
    b.toggle_tag("SOFT");
    assert_eq!(names(&b), ["Pad A", "Pad B"]);
    b.set_tab(Tab::Favourites);
    assert_eq!(names(&b), ["Pad A"]);
    b.set_tab(Tab::Mine);
    assert_eq!(names(&b), ["Pad A", "Pad B"]);
    b.set_tab(Tab::Factory);
    assert!(b.list().is_empty());
    b.toggle_tag("soft");
    assert_eq!(names(&b), ["Factory Pad"]);
    b.set_search("");
    b.set_tab(Tab::Mine);
    assert_eq!(names(&b), ["Edited Bass", "Pad A", "Pad B"]);
    b.set_tab(Tab::All);
    let mut all = entries();
    all[1].description = "a distant cloud".into();
    b.refresh(all);
    b.set_search("DISTANT soft");
    assert_eq!(names(&b), ["Pad B"]);
    b.toggle_tag("wide");
    assert!(b.list().is_empty());
}

#[test]
fn a_selected_tag_survives_when_no_entry_has_it() {
    let mut b = browser();
    b.toggle_tag("soft");
    b.refresh([preset("New", Origin::User, false, &["new"])]);
    assert!(b.list().is_empty());
    assert!(b.chips().iter().any(|(t, on)| t == "soft" && *on));
    b.toggle_tag("SOFT");
    assert_eq!(names(&b), ["New"]);
    assert!(!b.chips().iter().any(|(t, _)| t == "soft"));
}

/// R18's prompt lifetime: a filter cannot redirect an edit to another row.
#[test]
fn an_edit_stays_with_its_preset_through_filters_and_refresh() {
    let mut b = browser();
    b.set_current(Some("Pad B"));
    b.act(BrowserAction::Rename);
    replace(&mut b, "Pad Renamed");
    b.set_tab(Tab::Favourites);
    assert_eq!(b.selected_row(), None);
    assert!(matches!(b.prompt(), Some(Prompt::Rename { .. })));
    let mut refreshed = entries();
    refreshed.push(preset("Pad 0", Origin::User, false, &[]));
    b.refresh(refreshed);
    let r = request(b.key(BrowserKey::Enter));
    assert_eq!(
        r.operation,
        Operation::Rename {
            preset: PresetId {
                name: "Pad B".into(),
                origin: Origin::User
            },
            to: "Pad Renamed".into(),
        }
    );
    assert_eq!(
        b.complete(r.token, Ok(Completion::Current(Some("Pad Renamed".into())))),
        [BrowserEffect::Changed]
    );
    assert!(b.prompt().is_none());
    assert_eq!(b.current_name(), Some("Pad Renamed"));
    assert_eq!(b.focus(), Some(FieldId::Search));
}

#[test]
fn a_prompt_ends_when_its_preset_disappears_or_changes_origin() {
    for action in [
        BrowserAction::Rename,
        BrowserAction::Tags,
        BrowserAction::Delete,
    ] {
        let mut b = browser();
        b.set_current(Some("Pad B"));
        b.act(action.clone());
        b.refresh(entries().into_iter().filter(|e| e.name != "Pad B"));
        assert!(b.prompt().is_none());
        assert_eq!(b.focus(), Some(FieldId::Search));
        assert_eq!(b.current_name(), Some("Pad B"));
        assert!(b.current().is_none());
        b.refresh(entries());
        b.act(action);
        let mut changed = entries();
        changed[1].origin = Origin::Edited;
        b.refresh(changed);
        assert!(b.prompt().is_none());
        assert!(b.confirm().is_empty());
    }
}

#[test]
fn another_preset_ends_every_prompt_and_ignores_its_old_completion() {
    for action in [
        BrowserAction::Rename,
        BrowserAction::Tags,
        BrowserAction::Delete,
        BrowserAction::SaveAs,
    ] {
        let mut b = browser();
        b.set_current(Some("Pad A"));
        b.act(action);
        let r = request(b.confirm());
        b.set_current(Some("Pad B"));
        b.act(BrowserAction::Tags);
        assert!(
            b.complete(r.token, Ok(Completion::Current(Some("Old reply".into()))))
                .is_empty()
        );
        assert_eq!(b.current_name(), Some("Pad B"));
        assert!(matches!(b.prompt(), Some(Prompt::Tags { preset, .. }) if preset.name == "Pad B"));
    }
}

#[test]
fn tags_and_delete_requests_keep_the_identity_after_rows_move() {
    let mut b = browser();
    b.set_current(Some("Pad B"));
    b.act(BrowserAction::Tags);
    replace(&mut b, "soft, deep");
    let mut refreshed = entries();
    refreshed.push(preset("Before", Origin::User, false, &[]));
    b.refresh(refreshed);
    let r = request(b.confirm());
    assert!(
        matches!(r.operation, Operation::Tags { preset, text } if preset.name == "Pad B" && text == "soft, deep")
    );
    b.complete(r.token, Ok(Completion::Changed));
    assert_eq!(b.act(BrowserAction::Delete), [BrowserEffect::Changed]);
    assert!(matches!(b.prompt(), Some(Prompt::Delete { preset }) if preset.name == "Pad B"));
    b.set_search("factory");
    let r = request(b.key(BrowserKey::Enter));
    assert!(matches!(r.operation, Operation::Delete(preset) if preset.name == "Pad B"));
    assert!(
        b.act(BrowserAction::Delete).is_empty(),
        "pending confirmation cannot submit twice"
    );
    b.complete(r.token, Ok(Completion::Changed));
    assert!(b.prompt().is_none());
}

#[test]
fn failed_requests_keep_the_field_and_a_retry_gets_a_new_token() {
    let mut b = browser();
    b.set_current(Some("Pad A"));
    b.act(BrowserAction::Rename);
    replace(&mut b, "New name");
    let first = request(b.confirm());
    b.key(BrowserKey::Text(TextEdit::Insert(" ignored while pending")));
    assert_eq!(b.field(FieldId::Edit).expect("field").text(), "New name");
    b.complete(first.token, Err("A name clash".into()));
    assert_eq!(b.error(), Some("A name clash"));
    assert_eq!(b.field(FieldId::Edit).expect("field").text(), "New name");
    replace(&mut b, "Another name");
    assert_eq!(b.error(), None);
    let retry = request(b.confirm());
    assert_ne!(first.token, retry.token);
    assert!(b.complete(first.token, Ok(Completion::Changed)).is_empty());
    assert_eq!(b.pending(), Some(&retry));
    b.complete(
        retry.token,
        Ok(Completion::Current(Some("Another name".into()))),
    );
    assert!(b.prompt().is_none() && b.pending().is_none());
    assert_eq!(b.current_name(), Some("Another name"));
}

#[test]
fn save_as_prefills_the_current_metadata_and_reports_replacement() {
    let mut b = browser();
    b.set_current(Some("Pad A"));
    b.act(BrowserAction::SaveAs);
    assert_eq!(b.field(FieldId::SaveName).expect("name").text(), "Pad A");
    assert_eq!(
        b.field(FieldId::SaveTags).expect("tags").text(),
        "soft, wide"
    );
    assert_eq!(b.replacement().expect("exists").name, "Pad A");
    replace(&mut b, " factory pad ");
    assert_eq!(b.replacement().expect("exists").origin, Origin::Factory);
    b.key(BrowserKey::Tab);
    assert_eq!(b.focus(), Some(FieldId::SaveTags));
    replace(&mut b, "warm, wide");
    let r = request(b.act(BrowserAction::SaveAs));
    assert_eq!(
        r.operation,
        Operation::SaveAs {
            name: "factory pad".into(),
            tags: "warm, wide".into()
        }
    );
    b.complete(r.token, Ok(Completion::Current(Some("Factory Pad".into()))));
    assert_eq!(b.focus(), Some(FieldId::Search));
    assert!(b.prompt().is_none());
    b.set_current(None);
    b.act(BrowserAction::SaveAs);
    assert!(
        b.confirm()
            .iter()
            .all(|e| !matches!(e, BrowserEffect::Request(_)))
    );
    assert_eq!(b.focus(), Some(FieldId::SaveName));
}

#[test]
fn escape_takes_back_a_prompt_then_closes_and_releases_the_focus() {
    let mut b = browser();
    b.set_current(Some("Pad A"));
    b.act(BrowserAction::Tags);
    b.key(BrowserKey::Escape);
    assert!(b.prompt().is_none() && b.is_open());
    assert_eq!(b.focus(), Some(FieldId::Search));
    assert!(b.key(BrowserKey::Escape).contains(&BrowserEffect::Close));
    assert!(!b.is_open());
    assert_eq!(b.focus(), None);
    assert!(
        b.key(BrowserKey::Text(TextEdit::Insert("host's key")))
            .is_empty()
    );
    b.show(true);
    assert_eq!(b.focus(), Some(FieldId::Search));
    assert!(b.key(BrowserKey::Enter).contains(&BrowserEffect::Close));
}

#[test]
fn choosing_a_row_ends_its_prompt_and_a_double_click_also_closes() {
    let mut b = browser();
    b.set_current(Some("Pad A"));
    b.act(BrowserAction::Delete);
    assert!(
        b.act(BrowserAction::Choose {
            row: usize::MAX,
            close: true
        })
        .is_empty()
    );
    assert!(b.prompt().is_some() && b.is_open());
    let effects = b.act(BrowserAction::Choose {
        row: 1,
        close: false,
    });
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, BrowserEffect::Choose(id) if id.name == "Factory Pad"))
    );
    assert!(b.prompt().is_none() && b.is_open());
    let effects = b.act(BrowserAction::Choose {
        row: 1,
        close: true,
    });
    assert!(effects.contains(&BrowserEffect::Close));
    assert!(!b.is_open() && b.focus().is_none());
}

#[test]
fn scrolling_uses_the_callers_visible_rows_and_saturates() {
    let mut b = browser();
    b.scroll(isize::MAX, 2);
    assert_eq!(b.first(), 2);
    b.scroll(isize::MIN, 2);
    assert_eq!(b.first(), 0);
    b.scroll(isize::MAX, 1);
    assert_eq!(b.first(), 3);
    b.set_visible_rows(99);
    assert_eq!(b.first(), 0);
    b.scroll(isize::MAX, 0);
    assert_eq!(b.first(), 0);
    b.scroll_tags(isize::MAX, 1, 1);
    assert_eq!(b.tags_first(), 2);
    b.scroll_tags(isize::MIN, 1, 1);
    assert_eq!(b.tags_first(), 0);
    b.scroll_tags(isize::MAX, 2, 1);
    assert_eq!(b.tags_first(), 1);
    b.scroll_tags(isize::MAX, 0, 1);
    assert_eq!(b.tags_first(), 0);
    b.refresh([]);
    b.scroll(isize::MAX, 1);
    assert_eq!(b.first(), 0);
}

#[test]
fn refresh_keeps_the_numeric_top_row_and_selection_tracks_its_name() {
    let mut b = browser();
    b.act(BrowserAction::Choose {
        row: 3,
        close: false,
    });
    assert_eq!((b.current_name(), b.first()), (Some("Pad B"), 2));
    let mut all = entries();
    all.push(preset("A new first row", Origin::User, false, &[]));
    b.refresh(all);
    assert_eq!(
        (b.current_name(), b.selected_row(), b.first()),
        (Some("Pad B"), Some(4), 2)
    );
    b.refresh(entries().into_iter().map(|mut e| {
        e.name = e.name.to_uppercase();
        e
    }));
    assert_eq!(b.current_name(), Some("Pad B"));
    assert_eq!(b.current().expect("found ignoring case").name, "PAD B");
}

#[test]
fn navigation_wraps_within_the_filtered_set_and_an_empty_set_changes_nothing() {
    let mut b = browser();
    b.set_search("pad");
    b.step(-1);
    assert_eq!(b.current_name(), Some("Pad B"));
    b.step(1);
    assert_eq!(b.current_name(), Some("Factory Pad"));
    b.step(isize::MIN);
    assert!(b.selected_row().is_some());
    b.step(isize::MAX);
    assert!(b.selected_row().is_some());
    b.set_search("missing");
    let previous = b.current_name().map(str::to_owned);
    assert!(b.step(1).is_empty());
    assert_eq!(b.current_name(), previous.as_deref());
    b.set_current(None);
    b.set_search("pad");
    b.key(BrowserKey::Next);
    assert_eq!(b.current_name(), Some("Factory Pad"));
    b.act(BrowserAction::Rename);
    assert!(b.key(BrowserKey::Next).is_empty());
}

#[test]
fn each_key_returns_the_callers_operation_and_revert_requires_an_edited_factory() {
    let mut b = browser();
    for action in [
        BrowserAction::Rename,
        BrowserAction::Tags,
        BrowserAction::Delete,
        BrowserAction::Revert,
    ] {
        assert!(
            b.act(action)
                .contains(&BrowserEffect::Unavailable(Unavailable::NoPreset))
        );
    }
    b.set_current(Some("Pad A"));
    assert!(
        b.act(BrowserAction::Revert)
            .contains(&BrowserEffect::Unavailable(Unavailable::NotEdited))
    );
    assert_eq!(b.focus(), Some(FieldId::Search));
    b.set_current(Some("Edited Bass"));
    let r = request(b.act(BrowserAction::Revert));
    assert!(matches!(r.operation, Operation::Revert(id) if id.name == "Edited Bass"));
    b.complete(
        r.token,
        Ok(Completion::Current(Some("Original Bass".into()))),
    );
    for (action, op) in [
        (BrowserAction::Restore, Operation::Restore),
        (BrowserAction::MidiLearn, Operation::MidiLearn),
        (BrowserAction::Init, Operation::Init),
        (BrowserAction::PresetPad(7), Operation::PresetPad(7)),
    ] {
        let r = request(b.act(action));
        assert_eq!(r.operation, op);
        b.complete(r.token, Ok(Completion::Changed));
    }
}

#[test]
fn favorite_completion_keeps_a_prompt_and_list_changes_are_only_reported_once() {
    let mut b = browser();
    assert!(b.list_changed().is_some());
    assert_eq!(b.list_changed(), None);
    b.set_current(Some("Pad A"));
    b.act(BrowserAction::Rename);
    let r = request(b.act(BrowserAction::Favorite(0)));
    assert_eq!(
        r.operation,
        Operation::Favorite {
            preset: PresetId {
                name: "Edited Bass".into(),
                origin: Origin::Edited
            },
            on: true
        }
    );
    b.complete(r.token, Ok(Completion::Changed));
    assert!(matches!(b.prompt(), Some(Prompt::Rename { .. })));
    let mut all = entries();
    all[0].favorite = false;
    b.refresh(all);
    assert_eq!(b.list_changed(), None);
    b.set_tab(Tab::Favourites);
    assert_eq!(b.list_changed(), Some(vec!["Factory Pad".into()]));
    assert_eq!(b.list_changed(), None);
}

#[test]
fn fields_share_unicode_carets_and_tab_visits_only_visible_fields() {
    let mut b = browser();
    assert!(!b.focus_field(FieldId::Edit, 0));
    b.key(BrowserKey::Text(TextEdit::Insert("Bà🦀")));
    assert!(b.focus_field(FieldId::Search, 2));
    b.key(BrowserKey::Text(TextEdit::Backspace));
    assert_eq!(b.field(FieldId::Search).expect("search").text(), "B🦀");
    assert_eq!(b.field(FieldId::Search).expect("search").caret(), 1);
    b.key(BrowserKey::Tab);
    assert_eq!(b.focus(), Some(FieldId::Search));
    b.set_current(Some("Pad A"));
    b.act(BrowserAction::Rename);
    b.key(BrowserKey::Tab);
    assert_eq!(b.focus(), Some(FieldId::Search));
    b.key(BrowserKey::Tab);
    assert_eq!(b.focus(), Some(FieldId::Edit));
    b.act(BrowserAction::SaveAs);
    for expected in [FieldId::SaveTags, FieldId::Search, FieldId::SaveName] {
        b.key(BrowserKey::Tab);
        assert_eq!(b.focus(), Some(expected));
    }
}

#[test]
fn a_disappearing_favorite_request_does_not_cancel_another_presets_prompt() {
    let mut b = browser();
    b.set_current(Some("Pad A"));
    b.act(BrowserAction::Rename);
    let r = request(b.act(BrowserAction::Favorite(0)));
    b.refresh(entries().into_iter().filter(|e| e.name != "Edited Bass"));
    assert!(b.pending().is_none());
    assert!(matches!(b.prompt(), Some(Prompt::Rename { preset, .. }) if preset.name == "Pad A"));
    assert!(b.complete(r.token, Ok(Completion::Changed)).is_empty());
}

#[test]
fn missing_selected_chips_follow_present_sorted_tags() {
    let mut b = BrowserState::new(2);
    b.refresh([preset("One", Origin::User, false, &["apple", "zebra"])]);
    b.toggle_tag("apple");
    b.refresh([preset("Two", Origin::User, false, &["zebra"])]);
    assert_eq!(
        b.chips(),
        &[("zebra".into(), false), ("apple".into(), true)]
    );
}

#[test]
fn the_released_row_bound_prompt_is_an_explicit_option() {
    for row_bound in [false, true] {
        let mut b = browser();
        b.show(true);
        b.set_current(Some("Pad B"));
        b.set_prompts_require_visible_row(row_bound);
        b.act(BrowserAction::Rename);
        assert!(b.prompt().is_some());
        b.set_search("no matching preset");
        assert_eq!(b.prompt().is_none(), row_bound);
    }
}

#[test]
fn search_caret_commands_reset_the_list_even_without_a_text_change() {
    let mut b = browser();
    b.show(true);
    b.scroll(2, 2);
    assert!(b.first() > 0);
    b.key(BrowserKey::Text(TextEdit::Home));
    assert_eq!(b.first(), 0);
    b.scroll(2, 2);
    b.key(BrowserKey::Text(TextEdit::Backspace));
    assert_eq!(b.first(), 0);
}
#[test]
fn a_zero_step_still_selects_the_current_or_last_row_as_released() {
    let mut b = browser();
    assert!(
        b.step(0)
            .iter()
            .any(|e| matches!(e, BrowserEffect::Choose(_)))
    );
    let current = b.current_name().unwrap().to_owned();
    assert!(
        b.step(0)
            .iter()
            .any(|e| matches!(e, BrowserEffect::Choose(id) if id.name == current))
    );
}

#[test]
fn ending_a_prompt_preserves_an_unfocused_released_drawer() {
    let mut b = browser();
    b.show(true);
    b.focus = None;
    b.end_prompt();
    assert_eq!(b.focus(), None);
    b.act(BrowserAction::Choose {
        row: 0,
        close: false,
    });
    assert_eq!(b.focus(), None);
    b.act(BrowserAction::Rename);
    b.end_prompt();
    assert_eq!(b.focus(), Some(FieldId::Search));
    b.show(false);
    b.show(true);
    assert_eq!(b.focus(), Some(FieldId::Search));
}
