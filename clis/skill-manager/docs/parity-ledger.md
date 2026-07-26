# Python parity traceability ledger

This ledger covers the committed inventory generated from `toolbelt/tests/test_skill_manager.py` exactly once. “Merged” means the listed focused Rust contract test includes the behavior in a parameterized or end-to-end scenario. Replacements are deliberate, documented behavior changes.

| Python test | Disposition | Rust coverage or deviation |
| --- | --- | --- |
| `TestCliImprovements.test_no_command_defaults_to_status` | Merged | `operations_contract::no_command_defaults_to_status_and_every_json_line_has_the_envelope` |
| `TestCliImprovements.test_status_json_includes_name_label_and_id` | Merged | `operations_contract::status_json_preserves_stable_and_human_source_provenance` |
| `TestCliImprovements.test_target_override_selects_disabled_target` | Merged | `operations_contract::disabled_builtin_flags_fail_but_explicit_target_name_opts_in` |
| `TestCliImprovements.test_legacy_alias_migrates_to_required_name_and_label` | Merged | `config::tests::rich_v0_migration_preserves_sources_targets_and_unknown_fields` |
| `TestCliImprovements.test_source_add_accepts_positional_name` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestCliImprovements.test_source_add_rejects_positional_and_flag_name` | Merged | `cli_contract::source_add_name_forms_are_positional_or_flag_but_not_both` |
| `TestCliImprovements.test_source_add_accepts_repeatable_exclude_patterns` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestCliImprovements.test_source_update_accepts_exclude_and_clear` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestResolveRemoveArgs.test_plain_name_resolution` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestResolveRemoveArgs.test_single_skill_dir_resolution` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestResolveRemoveArgs.test_collection_dir_resolution` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestResolveRemoveArgs.test_mixed_resolution` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestResolveRemoveArgs.test_empty_collection_prints_warning` | Replaced (D-012) | `D-012` |
| `TestRemoveSkillFromTarget.test_removes_existing_skill` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestRemoveSkillFromTarget.test_returns_false_when_not_found` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestRemoveSkillFromTarget.test_dry_run_does_not_delete` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestRemoveSkillFromTarget.test_dry_run_output_contains_dry_run_marker` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestRemoveDryRun.test_dry_run_prints_plan_and_writes_nothing` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestRemoveWithYes.test_yes_removes_without_prompting` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestRemoveConfirmation.test_confirms_y_proceeds` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestRemoveConfirmation.test_confirms_n_aborts` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestRemoveMissingSkill.test_warns_and_continues` | Replaced (D-012) | `D-012` |
| `TestRemoveMissingSkill.test_all_missing_prints_nothing_to_remove` | Replaced (D-012) | `D-012` |
| `TestRemoveWithFilter.test_filter_restricts_removal` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestRemoveWithFilter.test_filter_no_matches_prints_message` | Replaced (D-012) | `D-012` |
| `TestRemoveTargetScoping.test_only_specified_target_is_affected` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestUpdateSkillToTarget.test_updates_existing_skill` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestUpdateSkillToTarget.test_skips_missing_skill` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestUpdateSkillToTarget.test_dry_run_existing_does_not_overwrite` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestUpdateSkillToTarget.test_dry_run_missing_prints_would_skip` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestUpdateSkillToTarget.test_updates_existing_skill_prints_updated` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestUpdateSkillToTarget.test_skips_missing_skill_prints_skipped` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestUpdate.test_updates_existing_skill_in_all_targets` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestUpdate.test_skips_skill_not_in_any_target` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestUpdate.test_per_target_update_and_skip` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestUpdate.test_filter_restricts_update` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestUpdate.test_dry_run_writes_nothing` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestUpdate.test_summary_output` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestUpdate.test_dry_run_summary_output` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestUpdate.test_target_dir_never_created` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestDirsEqual.test_identical_dirs_are_equal` | Merged | `behavior_contract::directory_equality_compares_relative_names_and_contents` |
| `TestDirsEqual.test_different_file_content_not_equal` | Merged | `behavior_contract::directory_equality_compares_relative_names_and_contents` |
| `TestDirsEqual.test_extra_file_in_source_not_equal` | Merged | `behavior_contract::directory_equality_compares_relative_names_and_contents` |
| `TestDirsEqual.test_extra_file_in_target_not_equal` | Merged | `behavior_contract::directory_equality_compares_relative_names_and_contents` |
| `TestDirsEqual.test_empty_dirs_are_equal` | Merged | `behavior_contract::directory_equality_compares_relative_names_and_contents` |
| `TestSkillState.test_up_to_date` | Merged | `behavior_contract::status_covers_all_four_states` |
| `TestSkillState.test_needs_update` | Merged | `behavior_contract::status_covers_all_four_states` |
| `TestSkillState.test_not_loaded_with_source` | Merged | `behavior_contract::status_covers_all_four_states` |
| `TestSkillState.test_no_connection` | Merged | `behavior_contract::status_covers_all_four_states` |
| `TestSkillState.test_not_loaded_no_source_no_deploy` | Merged | `behavior_contract::status_covers_all_four_states` |
| `TestStatus.test_shows_source_preamble` | Merged | `operations_contract::human_status_renders_compact_source_legend_table_and_plain_summary` |
| `TestStatus.test_no_skills_prints_message` | Merged | `operations_contract::human_status_renders_compact_source_legend_table_and_plain_summary` |
| `TestStatus.test_table_header_contains_target_keys` | Merged | `operations_contract::human_status_renders_compact_source_legend_table_and_plain_summary` |
| `TestStatus.test_status_columns_use_each_target_header_width` | Replaced (D-013) | `D-013` |
| `TestStatus.test_up_to_date_state` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestStatus.test_needs_update_state` | Merged | `behavior_contract::status_covers_all_four_states` |
| `TestStatus.test_not_loaded_state` | Replaced (D-013) | `D-013` |
| `TestStatus.test_no_connection_state` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestStatus.test_summary_line_printed` | Merged | `operations_contract::human_status_renders_compact_source_legend_table_and_plain_summary` |
| `TestStatus.test_summary_omits_unsourced_deployed_when_zero` | Merged | `operations_contract::human_status_renders_compact_source_legend_table_and_plain_summary` |
| `TestStatus.test_skills_sorted_alphabetically` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestStatus.test_target_flag_scoping` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestStatus.test_filter_restricts_source_skills` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestStatus.test_deployed_only_skill_shown_without_source` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestStatus.test_missing_target_dir_treated_as_not_loaded` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestStatus.test_all_four_states_in_one_run` | Replaced (D-013) | `D-013` |
| `TestOutHelper.test_normal_mode_prints_text` | Merged | `operations_contract::human_output_honors_color_policy_and_diagnostic_streams` |
| `TestOutHelper.test_normal_mode_suppresses_nothing` | Merged | `operations_contract::human_status_renders_compact_source_legend_table_and_plain_summary` |
| `TestOutHelper.test_json_mode_prints_record` | Merged | `operations_contract::no_command_defaults_to_status_and_every_json_line_has_the_envelope` |
| `TestOutHelper.test_json_mode_suppresses_text_without_record` | Merged | `operations_contract::no_command_defaults_to_status_and_every_json_line_has_the_envelope` |
| `TestOutHelper.test_json_mode_blank_line_suppressed` | Merged | `operations_contract::no_command_defaults_to_status_and_every_json_line_has_the_envelope` |
| `TestJsonOutputLoad.test_loaded_record_fields` | Merged | `operations_contract::skill_action_events_preserve_loaded_overwritten_updated_copied_and_removed_provenance` |
| `TestJsonOutputLoad.test_overwritten_action` | Merged | `operations_contract::skill_action_events_preserve_loaded_overwritten_updated_copied_and_removed_provenance` |
| `TestJsonOutputLoad.test_dry_run_field_true` | Merged | `operations_contract::dry_run_never_writes_deployments_or_configuration` |
| `TestJsonOutputLoad.test_warning_record_on_no_filter_match` | Replaced (D-012) | `D-012` |
| `TestJsonOutputLoad.test_output_is_valid_ndjson` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestJsonOutputUpdate.test_updated_and_skipped_records` | Merged | `operations_contract::skill_action_events_preserve_loaded_overwritten_updated_copied_and_removed_provenance` |
| `TestJsonOutputUpdate.test_summary_record_emitted` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestJsonOutputStatus.test_skill_record_with_targets` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestJsonOutputStatus.test_state_values_are_valid` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestJsonOutputStatus.test_summary_record_with_counts` | Merged | `operations_contract::no_command_defaults_to_status_and_every_json_line_has_the_envelope` |
| `TestJsonOutputStatus.test_resolved_sources_emit_summary_without_crashing` | Merged | `operations_contract::status_filter_sorting_and_target_scoping_are_deterministic` |
| `TestJsonOutputStatus.test_no_skills_emits_info_record` | Merged | `operations_contract::no_command_defaults_to_status_and_every_json_line_has_the_envelope` |
| `TestJsonOutputStatus.test_output_is_valid_ndjson` | Merged | `operations_contract::no_command_defaults_to_status_and_every_json_line_has_the_envelope` |
| `TestJsonOutputRemove.test_removed_record` | Merged | `operations_contract::skill_action_events_preserve_loaded_overwritten_updated_copied_and_removed_provenance` |
| `TestJsonOutputRemove.test_missing_skill_warning_record` | Replaced (D-012) | `D-012` |
| `TestJsonOutputRemove.test_nothing_to_remove_info_record` | Replaced (D-012) | `D-012` |
| `TestApplyJsonInput.test_filter_string_normalised_to_list` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_filter_list_preserved` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_source_string_normalised_to_list` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_source_list_preserved` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_all_targets_set_via_all_key` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_claude_flag_set` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_dry_run_set` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_yes_set` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_unknown_keys_ignored` | Replaced (D-001) | `D-001` |
| `TestApplyJsonInput.test_skills_string_normalised_to_list` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_cd_only_set` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_no_cd_set` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestApplyJsonInput.test_no_input_set` | Merged | `recipe_contract::recipe_overlay_covers_transfer_command_shapes` |
| `TestLoadConfig.test_returns_empty_dict_when_file_missing` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestLoadConfig.test_returns_parsed_content` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestLoadConfig.test_returns_empty_dict_on_invalid_json` | Replaced (D-002) | `D-002` |
| `TestLoadConfig.test_filters_fictional_octo_org_placeholder_sources` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestSaveConfig.test_creates_file_with_correct_content` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestSaveConfig.test_overwrites_existing_file` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestGetStoredDirs.test_returns_existing_paths_with_labels` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestGetStoredDirs.test_warns_and_skips_missing_dirs` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestGetStoredDirs.test_uses_dirname_as_default_label` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestGetStoredDirs.test_returns_empty_list_for_empty_config` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestGetStoredDirs.test_preserves_insertion_order` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestResolveSources.test_explicit_sources_returned_directly` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestResolveSources.test_default_returns_stored_only` | Merged | `operations_contract::cwd_source_selectors_change_discovery_without_reordering_configured_sources` |
| `TestResolveSources.test_cd_only_returns_only_cwd` | Merged | `operations_contract::cwd_source_selectors_change_discovery_without_reordering_configured_sources` |
| `TestResolveSources.test_no_cd_returns_only_stored` | Merged | `operations_contract::cwd_source_selectors_change_discovery_without_reordering_configured_sources` |
| `TestResolveSources.test_empty_config_default_returns_empty` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryAdd.test_adds_directory_to_config` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryAdd.test_errors_on_duplicate_path` | Merged | `operations_contract::source_validation_rejects_duplicates_unknowns_and_invalid_values` |
| `TestDirectoryAdd.test_prompts_for_name_then_label_when_not_provided` | Merged | `operations_contract::human_prompts_cover_text_confirmation_cancellation_and_invalid_answers` |
| `TestDirectoryAdd.test_uses_cwd_when_no_directory_given` | Merged | `operations_contract::source_add_and_remove_without_a_reference_use_the_current_directory` |
| `TestDirectoryAdd.test_resolves_symlinks_before_storing` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryRemove.test_removes_existing_entry` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryRemove.test_warns_when_path_not_stored` | Merged | `operations_contract::source_validation_rejects_duplicates_unknowns_and_invalid_values` |
| `TestDirectoryRemove.test_uses_cwd_when_no_directory_given` | Merged | `operations_contract::source_add_and_remove_without_a_reference_use_the_current_directory` |
| `TestDirectoryRemove.test_main_removes_github_source_by_name` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryRemove.test_removes_stored_source_by_unique_label` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryUpdate.test_updates_by_name_without_changing_source_id` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryUpdate.test_update_adds_excludes_without_name_or_label` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryUpdate.test_update_clear_exclude_then_adds_new_patterns` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryUpdate.test_updates_by_unique_label_without_changing_source_id` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestDirectoryList.test_prints_table_with_labels_and_paths` | Merged | `operations_contract::source_list_machine_and_empty_cases` |
| `TestDirectoryList.test_prints_message_when_empty` | Merged | `operations_contract::source_list_machine_and_empty_cases` |
| `TestDirectoryList.test_json_mode_emits_records` | Merged | `operations_contract::source_list_machine_and_empty_cases` |
| `TestDirectoryList.test_missing_dirs_marked_in_output` | Merged | `operations_contract::source_list_machine_and_empty_cases` |
| `TestDirectoryList.test_json_mode_empty_emits_info_record` | Merged | `operations_contract::source_list_machine_and_empty_cases` |
| `TestIsSkillName.test_bare_name_is_skill_name` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestIsSkillName.test_forward_slash_is_not_skill_name` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestIsSkillName.test_backslash_is_not_skill_name` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestIsSkillName.test_tilde_prefix_is_not_skill_name` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestIsSkillName.test_dot_prefix_is_not_skill_name` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestIsSkillName.test_windows_drive_letter_is_not_skill_name` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestIsSkillName.test_single_char_is_skill_name` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestFindSkillByName.test_finds_skill_in_single_dir` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestFindSkillByName.test_finds_skill_in_multiple_dirs` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestFindSkillByName.test_returns_empty_when_not_found` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestFindSkillByName.test_ignores_dir_without_skill_md` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestPickSkillMatch.test_single_match_returned_directly` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestPickSkillMatch.test_zero_matches_exits` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestPickSkillMatch.test_multiple_matches_json_mode_errors` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestPickSkillMatch.test_multiple_matches_interactive_default` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestPickSkillMatch.test_multiple_matches_interactive_second` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestSourceLabel.test_returns_config_label_for_stored_dir` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestSourceLabel.test_returns_config_label_for_subdir_of_stored_dir` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestSourceLabel.test_returns_current_directory_for_unknown_path` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestResolveSourcesBareNames.test_bare_name_found_in_stored_dir` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveSourcesBareNames.test_bare_name_found_in_materialised_github_source` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveSourcesBareNames.test_bare_name_not_found_exits` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveSourcesBareNames.test_explicit_path_not_searched` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveSourcesBareNames.test_bare_name_cd_only_searches_cwd` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveSourcesBareNames.test_bare_name_cd_only_not_in_cwd_exits` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveSourcesBareNames.test_bare_name_default_does_not_search_cwd` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveRemoveArgsBareNames.test_bare_name_found_in_search_dirs` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveRemoveArgsBareNames.test_bare_name_not_in_search_dirs_falls_back_to_plain_name` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveRemoveArgsBareNames.test_path_arg_still_works_with_search_dirs` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestResolveRemoveArgsBareNames.test_no_search_dirs_falls_back_to_plain_name` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestConfirmAllTargets.test_shows_source_label_from_config` | Replaced (D-011) | `D-011` |
| `TestConfirmAllTargets.test_shows_current_directory_for_cwd` | Replaced (D-011) | `D-011` |
| `TestConfirmAllTargets.test_skill_dir_shown_as_name_with_label` | Replaced (D-011) | `D-011` |
| `TestConfirmAllTargets.test_dry_run_skips_prompt` | Merged | `operations_contract::dry_run_never_writes_deployments_or_configuration` |
| `TestConfirmAllTargets.test_no_sources_no_from_section` | Replaced (D-011) | `D-011` |
| `TestNoInputMode.test_pick_skill_match_errors_on_ambiguity` | Merged | `operations_contract::interactive_collision_choice_selects_the_requested_winner` |
| `TestNoInputMode.test_pick_skill_match_lists_candidates_in_error` | Merged | `operations_contract::interactive_collision_choice_selects_the_requested_winner` |
| `TestNoInputMode.test_pick_skill_match_single_match_unchanged` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestNoInputMode.test_confirm_all_targets_errors_in_no_input_mode` | Merged | `operations_contract::human_prompts_cover_text_confirmation_cancellation_and_invalid_answers` |
| `TestNoInputMode.test_confirm_all_targets_dry_run_skips_error` | Merged | `operations_contract::human_prompts_cover_text_confirmation_cancellation_and_invalid_answers` |
| `TestNoInputMode.test_remove_errors_on_confirmation_without_yes` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestNoInputMode.test_remove_proceeds_when_yes_and_no_input` | Merged | `operations_contract::copy_load_update_status_and_remove_mutate_expected_trees` |
| `TestNoInputMode.test_directory_add_errors_without_name` | Merged | `operations_contract::machine_input_carriers_are_exclusive_and_noninteractive` |
| `TestNoInputMode.test_directory_add_proceeds_with_label_in_no_input_mode` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestNoInputMode.test_directory_add_derives_label_in_no_input_mode` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestNoInputMode.test_directory_add_persists_repeatable_excludes` | Merged | `operations_contract::source_lifecycle_persists_updates_and_removal` |
| `TestNoInputMode.test_json_mode_pick_skill_errors_on_ambiguity` | Merged | `operations_contract::interactive_collision_choice_selects_the_requested_winner` |
| `TestNoInputMode.test_json_mode_confirm_all_targets_errors` | Merged | `operations_contract::human_prompts_cover_text_confirmation_cancellation_and_invalid_answers` |
| `TestNoInputMode.test_json_mode_remove_errors_without_yes` | Merged | `operations_contract::filters_update_only_and_remove_confirmation_preserve_unselected_content` |
| `TestMigrateConfigIfNeeded.test_migrate_success` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestMigrateConfigIfNeeded.test_migrate_skipped_when_new_already_exists` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestMigrateConfigIfNeeded.test_migrate_skipped_when_no_legacy` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestMigrateConfigIfNeeded.test_migrate_fallback_warns_on_failure` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestMigrateConfigIfNeeded.test_load_config_triggers_migration` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestMigrateConfigIfNeeded.test_save_config_triggers_migration` | Merged | `operations_contract::legacy_config_migrates_with_backup_and_dry_run_stays_in_memory` |
| `TestSourceParsing.test_parses_github_tree_url` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestSourceParsing.test_parses_github_shorthand_with_ref_and_path` | Merged | `behavior_contract::source_reference_and_bare_name_resolution_cases` |
| `TestCollisionHandling.test_discovery_tracks_collisions_and_keeps_first_source` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestCollisionHandling.test_discovery_applies_source_excludes_case_insensitively` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestCollisionHandling.test_resolve_command_persists_excludes_for_non_selected_source` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |
| `TestCollisionHandling.test_resolve_no_input_requires_prefer_source` | Merged | `operations_contract::collisions_are_first_source_wins_and_resolve_persists_exclude` |

## Intentional deviations

| ID | Python behavior | Rust canonical behavior | Rationale |
| --- | --- | --- | --- |
| D-001 | JSON recipe input silently ignored unknown fields. | Unknown fields are rejected as invalid input. | Strict recipes catch misspellings and make automation safe. |
| D-002 | Invalid config JSON was treated as an empty config. | Malformed or type-invalid config fails without rewriting it. | A damaged configuration must never be silently replaced or hidden. |
| D-011 | Target confirmation rendered a source-by-source “from” preamble with labels and CWD special cases. | Interactive load/update asks one deterministic aggregate confirmation for all enabled targets; dry-run does not prompt. | The canonical prompt answers the safety-critical target decision without duplicating status/source rendering. |
| D-012 | Empty-source, unmatched-filter, and missing-remove cases emitted command-specific warning/info prose records. | Canonical commands exit successfully and finish with a typed zero-count `summary` event. | One deterministic no-work contract is easier for automation to consume than command-specific prose diagnostics. |
| D-013 | Status always rendered emoji cells and legends, including when output was redirected. | Interactive terminals retain emoji markers; non-TTY human output uses plain textual states with no ANSI or emoji. | Redirected output must be stable, searchable plain text as required by the canonical stream contract. |

The broader canonical changes—including disabled target lifecycle, strict JSON input/output, non-TTY stream behavior, cache TTL/token policy, Unicode case-folding, safe archive/tree validation, journaled transactions, and no-write dry-runs—are enumerated in [deviations.md](deviations.md). They strengthen merged coverage rather than replacing a Python test expectation.
