// Included inside mixer::tests so the loaded dictionary exercises the original
// assertions as well as the default test run's compiled fallback. Timing has its
// own benchmark; replaying the synthetic 10k-entry latency fixture adds no coverage.
pub(super) fn check_loaded_dictionary(index: Arc<crate::zhixu::Index>) {
    let mut failures = Vec::new();
    let mut passed = 0;
    macro_rules! replay {
        ($($test:path),* $(,)?) => {$(
            match std::panic::catch_unwind(|| crate::zhixu::with_index(index.clone(), $test)) {
                Ok(()) => passed += 1,
                Err(_) => failures.push(stringify!($test)),
            }
        )*};
    }
    replay!(
        frozen_hr1_through_hr8_replay_is_green,
        typo_distance_covers_transpose_missing_extra_and_substitution,
        hr1_blocked_text_is_removed_from_every_source,
        hr2_fixed_prefix_is_tier_zero_and_hr3_deduplicates_sources,
        hr4_empty_personalization_is_byte_for_byte_base_and_hr5_is_deterministic,
        host_context_order_preserves_slots_selection_and_learning_text,
        complete_group_admits_abbreviated_full_readings_and_promotion_only,
        host_context_rejects_invalid_permutations_atomically,
        host_context_order_expires_on_input_and_paging,
        host_context_excludes_abbreviations_partial_english_and_unverified_readings,
        context_model_reorders_only_existing_candidates_and_preserves_selection_mapping,
        unmatched_or_removed_context_model_degrades_to_exact_base_order,
        nonempty_input_never_exposes_an_empty_first_candidate,
        fuzzy_pool_stably_prioritizes_exact_readings_without_hiding_expansions,
        coherent_nonexact_readings_beat_abbreviation_assembly_without_disabling_initials,
        owner_verified_raw_hit_commits_complete_text_locally_without_generalizing,
        long_dictionary_phrases_allow_one_shortened_syllable_without_promoting_garble,
        complete_dictionary_phrase_owns_only_top1_over_partial_prefixes,
        registered_ascii_exact_blocks_only_nonexact_chinese_promotion,
        heavy_garble_uses_bounded_indexed_edits_and_the_registered_ascii_gate,
        traditional_and_variant_characters_never_join_a_model_group_below_the_first,
        letters_that_spell_several_words_alike_group_them,
        an_offered_composition_takes_the_last_first_page_slot_and_commits,
        the_single_characters_after_two_pages_follow_the_letters_left_after_a_picked_segment,
        what_the_composition_offer_pushes_off_the_first_page_leads_the_second,
        what_the_offer_pushes_off_a_last_page_is_one_more_page,
        ai_era_names_are_registered_with_their_official_spelling,
        apostrophes_and_umlauts_do_not_lose_letters_of_the_composition,
        a_segment_picked_mid_composition_is_kept_and_not_spelled_again,
        model_composed_sentences_are_offered_and_lead_only_when_asked,
        loose_slips_fix_invalid_spellings_third_and_spare_abbreviations_and_particles,
        bare_initial_groups_same_syllable_characters_only_with_context,
        valid_spelling_slip_offers_the_dictionary_word_only_for_composed_firsts,
        english_committed_with_enter_is_remembered,
        spelling_repairs_swap_and_double_but_never_drop_a_consonant,
        english_words_pinyin_cannot_spell_lead_verbatim,
        deliberate_english_picks_lead_until_chinese_is_picked,
        mixed_language_promotes_bad_pinyin_and_keeps_normal_chinese_order,
        mixed_language_preserves_coherent_abbreviation_repairs_and_base_learning_path,
        singles_after_two_pages_follow_how_the_input_starts,
        three_letter_english_takes_the_last_first_page_slot_and_never_leads,
        raw_ascii_fallback_takes_the_last_first_page_slot_and_terms_stay,
        mixed_language_quality_rule_is_deterministic,
        narrow_mixed_input_embeds_one_unambiguous_canonical_ascii_term,
        offcorpus_ascii_is_generalized_and_chinese_initials_stay_first,
        short_tech_terms_never_leak_into_the_prediction_only_second_row,
        hr6_one_selection_moves_at_most_three_and_never_drops_without_action,
        learned_promotions_remain_bounded_and_fade_only_with_time,
        withdraw_selection_restores_persisted_snapshot_and_is_idempotent,
        withdraw_selection_restores_fifo_and_expired_entries,
        withdraw_selection_only_latest_and_forget_invalidates,
        a_pick_made_three_times_goes_first_whatever_its_slot,
        learned_promotions_survive_a_restart_and_forgetting_reaches_the_file,
        top_one_is_committed_without_learning_but_deliberate_selection_learns,
        blacklist_is_reading_scoped_filters_every_source_and_forgets_learning,
        backspace_on_non_ascii_override_shortens_by_char_without_panicking,
        typo_dictionary_is_loaded_and_nonempty_from_the_frozen_corpus,
        typo_corrections_remain_internal_and_never_enter_candidate_rows,
        hr8_project_terms_only_apply_to_the_exact_active_project,
        generated_project_abbreviation_is_scoped_and_does_not_change_existing_repairs,
        project_profile_activates_only_its_packs_and_global_order_is_stable,
        secondary_layer_never_uses_typo_or_raw_ascii_as_filler,
        local_prediction_is_opt_in_secondary_only_and_never_takes_numbered_slots,
        an_associated_phrase_after_a_picked_segment_is_selected_by_the_letters_left,
        secondary_local_selection_clears_digit_extension_raw_override,
        confident_auto_profile_promotes_exact_and_can_complete_in_prediction_layer,
        owner_declared_disambiguation_promotes_the_matching_long_phrase,
        personalized_overlay_is_not_repeated_on_later_base_pages,
        deep_page_fallback_preserves_first_two_pages_and_full_set_without_duplicates,
        short_reading_keeps_every_native_page_unchanged,
        peeking_following_pages_matches_paging_and_leaves_the_page_alone,
        peek_following_pages_from_a_kept_column_returns_to_the_same_page_and_highlight,
        fuzzy_and_deep_selection_clamps_within_current_page,
        fuzzy_linear_selection_crosses_pages_in_both_directions,
        deep_linear_selection_crosses_pages_and_returns_to_native_page,
        linear_return_does_not_reapply_host_reordering_to_landing_page,
        fuzzy_explicit_paging_crosses_pages_in_both_directions,
        deep_explicit_paging_crosses_pages_and_returns_to_native_page,
        fuzzy_long_reading_keeps_first_two_pages_and_prioritizes_deep_single_characters,
        fuzzy_abbreviation_keeps_the_exact_spelling_word_ahead_of_the_fuzzy_derivation,
        fuzzy_exact_protection_keeps_a_post_july_exact_word_first,
        one_hundred_personal_queries_meet_the_frozen_top_one_threshold,
        frozen_typo_set_meets_eighty_percent_hit_rate_deterministically,
        typo_enabled_has_zero_base_first_layer_regressions,
        linear_selection_reaches_offered_last_page_overflow_and_returns,
        ordinary_selection_on_offered_last_page_keeps_native_behavior,
        offered_last_page_snapshot_and_peek_keep_non_linear_semantics,
        linear_overflow_remains_reachable_with_native_last_page_flag,
        windows_peek_does_not_expand_the_offered_native_last_page,
        super::composition_probe::the_lattice_keeps_homophones_and_diverse_readings_the_decoder_cannot,
        super::composition_probe::compositions_include_the_natural_reading,
    );
    eprintln!(
        "loaded_dictionary_regressions: {passed} passed, {} failed",
        failures.len()
    );
    for name in &failures {
        eprintln!("loaded_dictionary_regression_failed: {name}");
    }
    assert!(
        failures.is_empty(),
        "loaded runtime dictionary changed existing assertions: {failures:?}"
    );
}

#[test]
fn dictionary_swap_keeps_the_displayed_selection_until_the_next_edit() {
    use crate::zhixu::{
        self, Index,
        loader::{DictionaryData, Entry},
    };
    let before = Arc::new(Index::from_data(DictionaryData {
        entries: vec![Entry {
            text: "QuantumCat".into(),
            reading: "quantumcat".into(),
            weight: 800,
            layer: "downloaded".into(),
        }],
    }));
    let after = Arc::new(Index::from_data(DictionaryData::default()));
    let mut mixer = MixerDecoder::new(
        Box::new(ListDecoder::with("quantumcat", &[("旧候选", "quantumcat")])),
        "runtime-swap".into(),
    );
    let displayed = zhixu::with_index(before, || mixer.snapshot());
    assert_eq!(displayed.candidates[0].text, "QuantumCat");
    zhixu::with_index(after, || {
        assert_eq!(
            mixer.snapshot().candidates,
            displayed.candidates,
            "a published index cannot change a displayed digit target"
        );
        // Every input/backspace clears this layer before obtaining the next page.
        mixer.reset_layer();
        assert!(
            !mixer
                .snapshot()
                .candidates
                .iter()
                .any(|candidate| candidate.text == "QuantumCat")
        );
    });
}

#[test]
fn dictionary_swap_preserves_the_displayed_words_abbreviation_gate() {
    use crate::zhixu::{
        self, Index,
        loader::{DictionaryData, Entry},
    };
    let before = Arc::new(Index::from_data(DictionaryData {
        entries: vec![Entry {
            text: "量子猫".into(),
            reading: "liang zi mao".into(),
            weight: 900,
            layer: "downloaded".into(),
        }],
    }));
    let after = Arc::new(Index::from_data(DictionaryData::default()));
    let mixer = MixerDecoder::new(
        Box::new(ListDecoder::with("liangzm", &[("量子猫", "liang zi mao")])),
        "runtime-known-word".into(),
    );
    assert!(!zhixu::with_index(before, || mixer.composition_guessed()));
    assert!(
        !zhixu::with_index(after, || mixer.composition_guessed()),
        "the displayed word keeps the same vocabulary protection until the next edit"
    );
}
