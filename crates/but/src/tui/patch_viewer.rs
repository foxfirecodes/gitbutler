//! Interactive patch-mode staging, similar to `git add -p`.
//!
//! Walks through all uncommitted hunks one at a time, prompting the user to stage (y),
//! skip (n), stage all remaining hunks in the current file (a), skip all remaining
//! hunks in the current file (d), or quit (q).

use bstr::BString;
use but_core::HunkHeader;
use colored::Colorize;

use super::diff_viewer::DiffLine;
use crate::tui::stage_viewer::StageFileEntry;

/// The result of running the interactive patch viewer.
pub(crate) enum PatchResult {
    /// User completed the review (possibly staging some hunks).
    Done {
        /// Hunks to assign to the target branch.
        selected: Vec<(Option<HunkHeader>, BString)>,
        /// Hunks to explicitly unassign (set to no branch).
        unselected: Vec<(Option<HunkHeader>, BString)>,
    },
}

/// Render a single diff hunk to the terminal with colored output.
fn render_hunk(diff_lines: &[DiffLine], out: &mut impl std::io::Write) -> std::io::Result<()> {
    for dl in diff_lines {
        match dl {
            DiffLine::HunkHeader(text) => {
                writeln!(out, "{}", text.cyan())?;
            }
            DiffLine::Added { line_num, content } => {
                writeln!(out, "{}", format!("+{content}").green())?;
                // Suppress unused variable warning
                let _ = line_num;
            }
            DiffLine::Removed { line_num, content } => {
                writeln!(out, "{}", format!("-{content}").red())?;
                let _ = line_num;
            }
            DiffLine::Context { content, .. } => {
                writeln!(out, " {content}")?;
            }
            DiffLine::Info(text) => {
                writeln!(out, "{}", text.yellow())?;
            }
        }
    }
    Ok(())
}

/// Run interactive patch-mode staging over the given files.
///
/// Walks through each file and each hunk, showing the diff and prompting
/// the user for a decision. Returns `PatchResult::Done` with the selected
/// and unselected hunks.
pub(crate) fn run_patch_viewer(
    files: Vec<StageFileEntry>,
    branch_name: &str,
    read: &mut impl std::io::BufRead,
    write: &mut impl std::io::Write,
) -> anyhow::Result<PatchResult> {
    let mut selected: Vec<(Option<HunkHeader>, BString)> = Vec::new();
    let mut unselected: Vec<(Option<HunkHeader>, BString)> = Vec::new();

    let total_hunks: usize = files.iter().map(|f| f.hunks.len()).sum();
    let mut hunk_number = 0;

    for file in &files {
        let file_hunk_count = file.hunks.len();
        let mut skip_rest_of_file = false;
        let mut stage_rest_of_file = false;

        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            hunk_number += 1;

            if skip_rest_of_file {
                unselected.push((hunk.hunk_header, hunk.path_bytes.clone()));
                continue;
            }

            if stage_rest_of_file {
                selected.push((hunk.hunk_header, hunk.path_bytes.clone()));
                continue;
            }

            // Show file path and hunk header
            writeln!(
                write,
                "{} ({}/{})",
                format!("--- {} ---", file.path).bold(),
                hunk_idx + 1,
                file_hunk_count,
            )?;
            render_hunk(&hunk.diff_lines, write)?;

            // Prompt
            loop {
                write!(
                    write,
                    "{} ({}/{}) {}",
                    "Stage this hunk".bold(),
                    hunk_number,
                    total_hunks,
                    format!("[y,n,a,d,q,?] → {branch_name}: ").dimmed(),
                )?;
                write.flush()?;

                let mut input = String::new();
                read.read_line(&mut input)?;
                let choice = input.trim().to_lowercase();

                match choice.as_str() {
                    "y" => {
                        selected.push((hunk.hunk_header, hunk.path_bytes.clone()));
                        break;
                    }
                    "n" => {
                        unselected.push((hunk.hunk_header, hunk.path_bytes.clone()));
                        break;
                    }
                    "a" => {
                        // Stage this hunk and all remaining hunks in the file
                        selected.push((hunk.hunk_header, hunk.path_bytes.clone()));
                        stage_rest_of_file = true;
                        break;
                    }
                    "d" => {
                        // Skip this hunk and all remaining hunks in the file
                        unselected.push((hunk.hunk_header, hunk.path_bytes.clone()));
                        skip_rest_of_file = true;
                        break;
                    }
                    "q" => {
                        // Mark current and all remaining hunks as unselected, then return
                        unselected.push((hunk.hunk_header, hunk.path_bytes.clone()));
                        // Add remaining hunks in this file
                        for remaining_hunk in file.hunks.iter().skip(hunk_idx + 1) {
                            unselected.push((
                                remaining_hunk.hunk_header,
                                remaining_hunk.path_bytes.clone(),
                            ));
                        }
                        // Add all hunks from remaining files
                        let file_idx = files
                            .iter()
                            .position(|f| std::ptr::eq(f, file))
                            .unwrap_or(0);
                        for remaining_file in files.iter().skip(file_idx + 1) {
                            for remaining_hunk in &remaining_file.hunks {
                                unselected.push((
                                    remaining_hunk.hunk_header,
                                    remaining_hunk.path_bytes.clone(),
                                ));
                            }
                        }
                        return Ok(PatchResult::Done {
                            selected,
                            unselected,
                        });
                    }
                    "?" | "" => {
                        writeln!(write, "{}", "y - stage this hunk".dimmed())?;
                        writeln!(write, "{}", "n - do not stage this hunk".dimmed())?;
                        writeln!(
                            write,
                            "{}",
                            "a - stage this hunk and all remaining hunks in the file".dimmed()
                        )?;
                        writeln!(
                            write,
                            "{}",
                            "d - do not stage this hunk or any remaining hunks in the file"
                                .dimmed()
                        )?;
                        writeln!(
                            write,
                            "{}",
                            "q - quit; do not stage this hunk or any remaining hunks".dimmed()
                        )?;
                        // Loop again for input
                    }
                    _ => {
                        writeln!(write, "{}", "Invalid input. Type '?' for help.".yellow())?;
                        // Loop again for input
                    }
                }
            }
        }
    }

    Ok(PatchResult::Done {
        selected,
        unselected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a simple hunk entry for testing.
    fn make_hunk(
        path: &str,
        old_start: u32,
        new_start: u32,
    ) -> crate::tui::stage_viewer::StageHunkEntry {
        let header = HunkHeader {
            old_start,
            old_lines: 3,
            new_start,
            new_lines: 4,
        };
        crate::tui::stage_viewer::StageHunkEntry {
            hunk_header: Some(header),
            path_bytes: BString::from(path),
            selected: true,
            diff_lines: vec![
                DiffLine::HunkHeader(format!("@@ -{},{} +{},{} @@", old_start, 3, new_start, 4)),
                DiffLine::Context {
                    old_num: old_start,
                    new_num: new_start,
                    content: "context line".to_string(),
                },
                DiffLine::Removed {
                    line_num: old_start + 1,
                    content: "old line".to_string(),
                },
                DiffLine::Added {
                    line_num: new_start + 1,
                    content: "new line".to_string(),
                },
                DiffLine::Added {
                    line_num: new_start + 2,
                    content: "another new line".to_string(),
                },
                DiffLine::Context {
                    old_num: old_start + 2,
                    new_num: new_start + 3,
                    content: "more context".to_string(),
                },
            ],
        }
    }

    fn make_files() -> Vec<StageFileEntry> {
        vec![
            StageFileEntry {
                path: "src/foo.rs".to_string(),
                hunks: vec![
                    make_hunk("src/foo.rs", 1, 1),
                    make_hunk("src/foo.rs", 20, 21),
                ],
            },
            StageFileEntry {
                path: "src/bar.rs".to_string(),
                hunks: vec![make_hunk("src/bar.rs", 5, 5)],
            },
        ]
    }

    #[test]
    fn stage_all_with_y() {
        let files = make_files();
        let mut input = b"y\ny\ny\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 3, "all 3 hunks should be staged");
                assert_eq!(unselected.len(), 0, "no hunks should be skipped");
            }
        }
    }

    #[test]
    fn skip_all_with_n() {
        let files = make_files();
        let mut input = b"n\nn\nn\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 0, "no hunks should be staged");
                assert_eq!(unselected.len(), 3, "all 3 hunks should be skipped");
            }
        }
    }

    #[test]
    fn mixed_y_and_n() {
        let files = make_files();
        let mut input = b"y\nn\ny\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 2);
                assert_eq!(unselected.len(), 1);
                // First hunk of foo staged, second skipped, bar staged
                assert_eq!(selected[0].1.as_slice(), b"src/foo.rs");
                assert_eq!(unselected[0].1.as_slice(), b"src/foo.rs");
                assert_eq!(selected[1].1.as_slice(), b"src/bar.rs");
            }
        }
    }

    #[test]
    fn stage_rest_of_file_with_a() {
        let files = make_files();
        // 'a' on first hunk of foo stages both foo hunks, then 'n' on bar
        let mut input = b"a\nn\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 2, "both foo hunks should be staged");
                assert_eq!(unselected.len(), 1, "bar hunk should be skipped");
                assert_eq!(selected[0].1.as_slice(), b"src/foo.rs");
                assert_eq!(selected[1].1.as_slice(), b"src/foo.rs");
                assert_eq!(unselected[0].1.as_slice(), b"src/bar.rs");
            }
        }
    }

    #[test]
    fn skip_rest_of_file_with_d() {
        let files = make_files();
        // 'd' on first hunk of foo skips both foo hunks, then 'y' on bar
        let mut input = b"d\ny\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 1, "only bar hunk should be staged");
                assert_eq!(unselected.len(), 2, "both foo hunks should be skipped");
                assert_eq!(selected[0].1.as_slice(), b"src/bar.rs");
            }
        }
    }

    #[test]
    fn quit_early_with_q() {
        let files = make_files();
        // 'y' on first foo hunk, then 'q' on second foo hunk
        let mut input = b"y\nq\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 1, "only first foo hunk should be staged");
                assert_eq!(
                    unselected.len(),
                    2,
                    "second foo hunk + bar hunk should be unselected"
                );
                assert_eq!(selected[0].1.as_slice(), b"src/foo.rs");
            }
        }
    }

    #[test]
    fn help_then_decide() {
        let files = make_files();
        // '?' shows help, then 'y' stages, then 'n', then 'y'
        let mut input = b"?\ny\nn\ny\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 2);
                assert_eq!(unselected.len(), 1);
            }
        }
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("stage this hunk"),
            "help text should be shown"
        );
    }

    #[test]
    fn invalid_input_prompts_again() {
        let files = make_files();
        // Invalid 'x', then 'y', 'y', 'y'
        let mut input = b"x\ny\ny\ny\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 3);
                assert_eq!(unselected.len(), 0);
            }
        }
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("Invalid input"),
            "invalid input message should be shown"
        );
    }

    #[test]
    fn empty_files() {
        let files: Vec<StageFileEntry> = Vec::new();
        let mut input = b"" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 0);
                assert_eq!(unselected.len(), 0);
            }
        }
    }

    #[test]
    fn single_hunk_stage() {
        let files = vec![StageFileEntry {
            path: "single.rs".to_string(),
            hunks: vec![make_hunk("single.rs", 1, 1)],
        }];
        let mut input = b"y\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 1);
                assert_eq!(unselected.len(), 0);
            }
        }
    }

    #[test]
    fn single_hunk_skip() {
        let files = vec![StageFileEntry {
            path: "single.rs".to_string(),
            hunks: vec![make_hunk("single.rs", 1, 1)],
        }];
        let mut input = b"n\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                assert_eq!(selected.len(), 0);
                assert_eq!(unselected.len(), 1);
            }
        }
    }

    #[test]
    fn output_contains_file_path() {
        let files = make_files();
        let mut input = b"y\ny\ny\n" as &[u8];
        let mut output = Vec::new();

        run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("src/foo.rs"),
            "output should show file path"
        );
        assert!(
            output_str.contains("src/bar.rs"),
            "output should show file path"
        );
    }

    #[test]
    fn output_contains_hunk_counters() {
        let files = make_files();
        let mut input = b"y\ny\ny\n" as &[u8];
        let mut output = Vec::new();

        run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("(1/2)"),
            "should show hunk position within file"
        );
        assert!(
            output_str.contains("(2/2)"),
            "should show hunk position within file"
        );
        assert!(output_str.contains("(1/1)"), "bar has only one hunk");
    }

    #[test]
    fn hunk_headers_preserved() {
        let files = make_files();
        let mut input = b"y\nn\ny\n" as &[u8];
        let mut output = Vec::new();

        let result = run_patch_viewer(files, "my-branch", &mut input, &mut output).unwrap();
        match result {
            PatchResult::Done {
                selected,
                unselected,
            } => {
                // First hunk: old_start=1
                let h1 = selected[0].0.unwrap();
                assert_eq!(h1.old_start, 1);
                // Second hunk (skipped): old_start=20
                let h2 = unselected[0].0.unwrap();
                assert_eq!(h2.old_start, 20);
                // Third hunk: old_start=5
                let h3 = selected[1].0.unwrap();
                assert_eq!(h3.old_start, 5);
            }
        }
    }
}
