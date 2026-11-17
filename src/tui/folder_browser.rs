//! Local folder browsing. Slow filesystem calls stay off the terminal thread.
use super::*;
use crate::folders::{Folder, Listing, Store};
use std::path::Path;

pub(super) enum FolderResult {
    Connected(Store, usize),
    Disconnected(Store, PathBuf),
    Listed(usize, Listing),
    Attached(pitty_core::Attachment),
    AttachedBatch(Vec<pitty_core::Attachment>, Option<String>),
    UnmatchedDrop(String),
}
#[derive(Clone, Copy)]
pub(super) enum FileChoice {
    Root(usize),
    Parent,
    Entry(usize),
}

impl App {
    fn folder_store(&self) -> Result<&Store, String> {
        self.folders.as_ref().map_err(Clone::clone)
    }
    fn folder_job(
        &mut self,
        work: impl FnOnce() -> Result<FolderResult, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.folder_rx.is_some() {
            return Err(
                "A folder operation is still finishing. You can keep editing or close Pitty."
                    .into(),
            );
        }
        let (tx, rx) = mpsc::channel();
        self.folder_rx = Some(rx);
        self.folder_abandoned = false;
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.status = "Accessing folder… Ctrl+C stops waiting; no upload.".into();
        Ok(())
    }
    pub(super) fn drop_in_background(&mut self, text: String) -> Result<(), String> {
        self.folder_job(move || {
            if let Some((paths, prompt)) = crate::attachments::dropped_paths(&text) {
                crate::attachments::load_batch(&paths)
                    .map(|files| FolderResult::AttachedBatch(files, Some(prompt)))
            } else {
                Ok(FolderResult::UnmatchedDrop(text))
            }
        })?;
        self.status = "Checking dropped paths… Ctrl+C cancels; nothing uploaded.".into();
        Ok(())
    }
    pub(super) fn attach_text_in_background(&mut self, text: String) -> Result<(), String> {
        self.folder_job(move || {
            let paths = crate::attachments::explicit_paths(&text)?;
            crate::attachments::load_batch(&paths)
                .map(|files| FolderResult::AttachedBatch(files, None))
        })?;
        self.status = "Reading selected files… Ctrl+C cancels staging; nothing uploaded.".into();
        Ok(())
    }
    pub(super) fn folder_command(&mut self, name: &str, arg: &str) -> Result<(), String> {
        match name {
            "connect" => {
                let paths = crate::attachments::path_tokens(arg)?;
                let mut store = self.folder_store()?.clone();
                self.folder_job(move || {
                    let added = store.connect(paths)?;
                    Ok(FolderResult::Connected(store, added))
                })?;
            }
            "disconnect" => {
                let mut store = self.folder_store()?.clone();
                let index = arg
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .filter(|&n| n < store.roots.len())
                    .ok_or("Use /disconnect NUMBER from /folders.")?;
                self.folder_job(move || {
                    let removed = store.disconnect(index)?;
                    Ok(FolderResult::Disconnected(store, removed.path))
                })?;
            }
            "folders" => self.show_folders()?,
            "files" => {
                if self.folder_store()?.roots.is_empty() {
                    return Err("Connect a folder first: /connect ~/Desktop/Project or /connect \"/Volumes/Shared Drive\".".into());
                }
                let root = if arg.is_empty() {
                    None
                } else {
                    Some(
                        arg.parse::<usize>()
                            .ok()
                            .and_then(|n| n.checked_sub(1))
                            .filter(|&n| n < self.folder_store().map_or(0, |s| s.roots.len()))
                            .ok_or("Use /files to browse, or /files NUMBER from /folders.")?,
                    )
                };
                if self.folder_rx.is_some() {
                    return Err("A folder operation is still finishing. Try /files again when it completes.".into());
                }
                self.open_picker(Picker::Files, "");
                self.browse_root = None;
                self.listing = None;
                if let Some(index) = root {
                    self.browse_folder(index, PathBuf::new())?;
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }
    fn show_folders(&mut self) -> Result<(), String> {
        let mut lines = vec!["/files opens the browser · /disconnect NUMBER forgets a folder".into(),
            "Connected folders are available to file tools when enabled. /tools controls local actions.".into(), String::new()];
        let store = self.folder_store()?;
        if store.roots.is_empty() {
            lines.push("No folders connected. Use /connect PATH [PATH ...].".into());
        }
        for (i, folder) in store.roots.iter().enumerate() {
            lines.push(format!("{}  {}", i + 1, folder.path.display()));
        }
        self.panel("Connected folders", lines);
        Ok(())
    }
    fn browse_folder(&mut self, index: usize, relative: PathBuf) -> Result<(), String> {
        let folder = self
            .folder_store()?
            .roots
            .get(index)
            .cloned()
            .ok_or("Folder is no longer connected.")?;
        self.folder_job(move || {
            Ok(FolderResult::Listed(
                index,
                crate::folders::list(&folder, &relative)?,
            ))
        })
    }
    pub(super) fn file_matches(&self) -> Vec<FileChoice> {
        let query = self.input.value().to_lowercase();
        if let Some(listing) = &self.listing {
            let mut choices = Vec::new();
            if query.is_empty() {
                choices.push(FileChoice::Parent);
            }
            choices.extend(
                listing
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.name.to_lowercase().contains(&query))
                    .map(|(i, _)| FileChoice::Entry(i)),
            );
            choices
        } else {
            self.folders
                .as_ref()
                .map(|s| {
                    s.roots
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| f.path.to_string_lossy().to_lowercase().contains(&query))
                        .map(|(i, _)| FileChoice::Root(i))
                        .collect()
                })
                .unwrap_or_default()
        }
    }
    fn file_parent(&mut self) -> Result<(), String> {
        let Some(index) = self.browse_root else {
            return Ok(());
        };
        let folder = self
            .folder_store()?
            .roots
            .get(index)
            .ok_or("Folder is no longer connected.")?;
        let Some(listing) = &self.listing else {
            return Ok(());
        };
        let relative = listing
            .directory
            .strip_prefix(&folder.path)
            .map_err(|_| "Folder location changed; reconnect it.")?;
        if relative.as_os_str().is_empty() {
            self.browse_root = None;
            self.listing = None;
            self.input.clear();
            self.picker_index = 0;
            self.status =
                "Choose a connected folder. Enter opens it; Esc returns to your draft.".into();
            Ok(())
        } else {
            self.browse_folder(
                index,
                relative.parent().unwrap_or(Path::new("")).to_path_buf(),
            )
        }
    }
    fn choose_file(&mut self) -> Result<(), String> {
        let Some(choice) = self.file_matches().get(self.picker_index).copied() else {
            return Ok(());
        };
        match choice {
            FileChoice::Root(index) => self.browse_folder(index, PathBuf::new()),
            FileChoice::Parent => self.file_parent(),
            FileChoice::Entry(index) => {
                let entry = &self
                    .listing
                    .as_ref()
                    .ok_or("Folder list is unavailable.")?
                    .entries[index];
                let root = self.browse_root.ok_or("Select a folder first.")?;
                let folder = self.folder_store()?.roots[root].clone();
                let path = entry.path.clone();
                if entry.is_dir {
                    let relative = path
                        .strip_prefix(&folder.path)
                        .map_err(|_| "Path is outside the connected folder.")?
                        .to_path_buf();
                    self.browse_folder(root, relative)
                } else {
                    self.folder_job(move || load_connected_file(folder, path))
                }
            }
        }
    }
    pub(super) fn handle_files(&mut self, key: Key) {
        // Esc detaches this browser from an in-flight result. OS filesystem calls
        // cannot always be interrupted; retain the receiver to bound worker count.
        if key == Key::Escape {
            self.close_picker();
            return;
        }
        let count = self.file_matches().len();
        let result = match key {
            Key::Up => {
                self.picker_index = self.picker_index.saturating_sub(1);
                Ok(())
            }
            Key::Down => {
                self.picker_index = (self.picker_index + 1).min(count.saturating_sub(1));
                Ok(())
            }
            Key::PageUp => {
                self.picker_index = self.picker_index.saturating_sub(self.picker_rows);
                Ok(())
            }
            Key::PageDown => {
                self.picker_index =
                    (self.picker_index + self.picker_rows).min(count.saturating_sub(1));
                Ok(())
            }
            Key::Enter if self.folder_rx.is_none() => self.choose_file(),
            Key::Left if self.input.is_empty() && self.folder_rx.is_none() => self.file_parent(),
            Key::Ctrl('r') if self.folder_rx.is_none() => {
                if let (Some(root), Some(listing)) = (self.browse_root, &self.listing) {
                    let relative = self
                        .folder_store()
                        .ok()
                        .and_then(|s| listing.directory.strip_prefix(&s.roots[root].path).ok())
                        .map(Path::to_path_buf);
                    relative.map_or(Ok(()), |p| self.browse_folder(root, p))
                } else {
                    Ok(())
                }
            }
            Key::Enter => Ok(()),
            Key::PasteRejected => Err("Paste exceeds 1 MB; nothing inserted.".into()),
            other => {
                self.input.edit(&other);
                self.picker_index = 0;
                Ok(())
            }
        };
        if let Err(e) = result {
            self.status = e;
        }
    }
    pub(super) fn receive_folder(&mut self) -> bool {
        let result = self.folder_rx.as_ref().and_then(|r| match r.try_recv() {
            Ok(v) => Some(v),
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Folder worker stopped unexpectedly.".into()))
            }
            _ => None,
        });
        let Some(result) = result else {
            return false;
        };
        self.folder_rx = None;
        let abandoned = self.folder_abandoned;
        self.folder_abandoned = false;
        match result {
            // A completed config write still applies after canceling the view.
            Ok(FolderResult::Connected(store, count)) => {
                self.folders = Ok(store);
                if !abandoned {
                    let _ = self.show_folders();
                    self.status = format!(
                        "{count} new folder(s) connected · /files to browse · nothing uploaded."
                    );
                }
            }
            Ok(FolderResult::Disconnected(store, path)) => {
                self.folders = Ok(store);
                self.listing = None;
                self.browse_root = None;
                if !abandoned {
                    let _ = self.show_folders();
                    self.status = format!(
                        "Disconnected {}. Files are unchanged; previously staged files remain staged.",
                        path.display()
                    );
                }
            }
            Ok(FolderResult::Listed(root, listing))
                if !abandoned && self.picker == Some(Picker::Files) =>
            {
                let note = if listing.truncated {
                    " · first 2,000 entries; open a subfolder to narrow"
                } else {
                    ""
                };
                self.status = format!(
                    "↑↓ choose · Enter open/attach · ← parent · Esc back{note} · {} skipped",
                    listing.skipped_entries
                );
                self.browse_root = Some(root);
                self.listing = Some(listing);
                self.input.clear();
                self.picker_index = 0;
            }
            Ok(FolderResult::Attached(file))
                if !abandoned && self.picker == Some(Picker::Files) =>
            {
                let name = file.name.clone();
                match self.stage_connected_file(file) {
                    Ok(()) => {
                        self.status = format!(
                            "Staged {name} · {} files ready · choose more or Esc to compose · nothing uploaded.",
                            self.attachments.len()
                        )
                    }
                    Err(e) => self.status = e,
                }
            }
            Ok(FolderResult::UnmatchedDrop(text)) if !abandoned => {
                self.input.insert(&text);
                self.checked_drop = Some(self.input.value());
                self.status =
                    "No existing file found. Text kept in your draft; Enter processes it normally."
                        .into();
            }
            Ok(FolderResult::AttachedBatch(files, prompt)) if !abandoned => {
                self.checked_drop = None;
                let original = self.attachments.len();
                let result = crate::attachments::stage_batch(&mut self.attachments, files);
                match result {
                    Ok(_) => {
                        if let Some(prompt) = prompt.filter(|s| !s.is_empty()) {
                            let mut draft = self.input.value();
                            if !draft.is_empty()
                                && !draft.ends_with(char::is_whitespace)
                                && !prompt.starts_with(char::is_whitespace)
                            {
                                draft.push(' ');
                            }
                            draft.push_str(&prompt);
                            self.input.set(&draft);
                        }
                        self.status = format!(
                            "{} new file(s) staged · {} total · Enter sends with your prompt; nothing uploaded.",
                            self.attachments.len() - original,
                            self.attachments.len()
                        );
                    }
                    Err(error) => {
                        self.attachments.truncate(original);
                        self.status = format!("{error} No files from this batch were staged.");
                    }
                }
            }
            Err(e) if !abandoned => {
                if self.picker == Some(Picker::Files) {
                    self.close_picker();
                }
                self.panel("Folder or file could not be accessed", vec![e.clone(), String::new(), "For a network drive, mount/reconnect it in your operating system, then use /files again.".into()]);
                self.status = e;
            }
            _ => {}
        }
        true
    }
    fn stage_connected_file(&mut self, file: pitty_core::Attachment) -> Result<(), String> {
        crate::attachments::stage(&mut self.attachments, file).map(|_| ())
    }
    pub(super) fn draw_files(
        &mut self,
        frame: &mut Frame,
        area: (usize, usize, usize, usize),
        colors: &Colors,
    ) {
        let (x, y, w, h) = area;
        if h == 0 {
            return;
        }
        let title = if let Some(listing) = &self.listing {
            format!("FILES · {}", listing.directory.display())
        } else {
            "CONNECTED FOLDERS · Enter to open".into()
        };
        frame.text(x, y, &clean(&title), colors.accent, w);
        let choices = self.file_matches();
        self.picker_index = self.picker_index.min(choices.len().saturating_sub(1));
        self.picker_rows = h.saturating_sub(4).max(1);
        let start = self.picker_index.saturating_sub(self.picker_rows - 1);
        for (row, choice) in choices
            .iter()
            .skip(start)
            .take(self.picker_rows)
            .enumerate()
        {
            if row + 2 >= h {
                break;
            }
            let label = match choice {
                FileChoice::Root(index) => self
                    .folder_store()
                    .map(|s| format!("{}  {}/", index + 1, s.roots[*index].path.display()))
                    .unwrap_or_default(),
                FileChoice::Parent => "../   Parent / connected folders".into(),
                FileChoice::Entry(index) => {
                    let e = &self.listing.as_ref().unwrap().entries[*index];
                    format!("{}{}", e.name, if e.is_dir { "/" } else { "" })
                }
            };
            let selected = start + row == self.picker_index;
            let style = if selected {
                colors.selected
            } else {
                colors.base
            };
            frame.fill(x, y + 2 + row, w, 1, style);
            frame.text(
                x,
                y + 2 + row,
                &format!("{} {}", if selected { "›" } else { " " }, clean(&label)),
                style,
                w,
            );
        }
        if choices.is_empty() && h > 2 {
            frame.text(
                x,
                y + 2,
                "No matches. Edit the filter or ← for parent.",
                colors.muted,
                w,
            );
        }
        if h > 3 {
            frame.text(
                x,
                y + h - 1,
                &format!(
                    "{} staged · {}",
                    self.attachments.len(),
                    if self.folder_rx.is_some() {
                        "Loading… Esc back"
                    } else {
                        "Select files locally; Esc returns to your message."
                    }
                ),
                colors.muted,
                w,
            );
        }
    }
}
fn load_connected_file(folder: Folder, path: PathBuf) -> Result<FolderResult, String> {
    let check = || -> Result<PathBuf, String> {
        let root = folder.path.canonicalize().map_err(|e| {
            format!(
                "Cannot access connected folder {}: {e}",
                folder.path.display()
            )
        })?;
        let resolved = path
            .canonicalize()
            .map_err(|e| format!("Cannot attach {}: {e}", path.display()))?;
        if root != folder.path || !resolved.starts_with(&root) {
            return Err(format!(
                "Cannot attach {}: target is outside or the connected folder {} changed. Reconnect it.",
                path.display(),
                folder.path.display()
            ));
        }
        Ok(resolved)
    };
    let resolved = check()?;
    let file = crate::attachments::load(&resolved)?;
    if check()? != resolved {
        return Err(format!(
            "Cannot attach {}: file location changed while reading; select it again.",
            path.display()
        ));
    }
    Ok(FolderResult::Attached(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "pitty-browser-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(path.join("Desktop Project/subfolder")).unwrap();
            fs::create_dir_all(path.join("Mounted Share")).unwrap();
            fs::write(path.join("Desktop Project/read me.rs"), "fn main() {}\n").unwrap();
            Self(path)
        }
        fn app(&self) -> App {
            let mut a = App::new(Args::try_parse_from(["pitty", "--no-history"]).unwrap());
            a.folders = Store::load_from(self.0.join("config/folders.json"));
            a
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn finish(a: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while a.folder_rx.is_some() && Instant::now() < deadline {
            a.receive();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(a.folder_rx.is_none(), "folder worker timed out");
    }
    #[test]
    fn connect_browse_stage_multiple_files_and_restore_draft_without_sending() {
        let f = Fixture::new();
        let mut a = f.app();
        a.input.set(&format!(
            "/connect '{}' '{}'",
            f.0.join("Desktop Project").display(),
            f.0.join("Mounted Share").display()
        ));
        a.send();
        finish(&mut a);
        assert_eq!(a.folder_store().unwrap().roots.len(), 2);
        assert!(a.attachments.is_empty());
        assert!(a.pending.is_none());
        a.input.set("Explain these files");
        a.command("files", "1").unwrap();
        finish(&mut a);
        assert!(a.picker == Some(Picker::Files));
        // Filter the listing without sending the filter to any model.
        a.input.set("subfolder");
        a.handle(Key::Enter);
        finish(&mut a);
        assert!(a.listing.as_ref().unwrap().directory.ends_with("subfolder"));
        a.handle(Key::Left);
        finish(&mut a);
        a.input.set("read me");
        a.handle(Key::Enter);
        finish(&mut a);
        assert_eq!(a.attachments.len(), 1);
        assert_eq!(a.attachments[0].name, "read me.rs");
        assert_eq!(a.attachments[0].data, "fn main() {}\n");
        assert!(a.pending.is_none());
        assert!(a.chats[a.active].messages.is_empty());
        a.handle(Key::Escape);
        assert_eq!(a.input.value(), "Explain these files");
        // Remembered roots survive application restarts; no mount probing needed.
        let mut restarted = f.app();
        assert_eq!(restarted.folder_store().unwrap().roots.len(), 2);
        fs::remove_dir_all(f.0.join("Mounted Share")).unwrap();
        restarted.command("files", "2").unwrap();
        finish(&mut restarted);
        assert!(restarted.status.contains("Mounted Share"));
        assert!(restarted.status.contains("unavailable"));
        restarted.command("disconnect", "2").unwrap();
        finish(&mut restarted);
        assert_eq!(restarted.folder_store().unwrap().roots.len(), 1);
        assert!(restarted.pending.is_none());
    }
    #[test]
    fn abandoned_file_read_never_stages_late_results_or_uploads() {
        let f = Fixture::new();
        let mut a = f.app();
        a.input.set("my unsent draft");
        a.open_picker(Picker::Files, "");
        let (tx, rx) = mpsc::channel();
        a.folder_rx = Some(rx);
        a.handle(Key::Escape);
        a.input.set("/tokens 1024");
        a.send();
        assert_eq!(
            a.args.max_tokens, 1024,
            "abandoned reads must not block local commands"
        );
        a.input.set("my unsent draft");
        a.handle(Key::Ctrl('c'));
        assert!(
            a.input.is_empty(),
            "abandoned reads must not prevent clearing a new draft"
        );
        a.input.set("my unsent draft");
        tx.send(Ok(FolderResult::Attached(pitty_core::Attachment {
            name: "secret.txt".into(),
            kind: "text".into(),
            mime: "text/plain".into(),
            data: "test".into(),
        })))
        .unwrap();
        assert!(a.receive());
        assert!(a.attachments.is_empty());
        assert!(a.pending.is_none());
        assert_eq!(a.input.value(), "my unsent draft");
    }
    #[test]
    fn unsupported_file_error_names_the_exact_path() {
        let f = Fixture::new();
        let root = f.0.join("Desktop Project").canonicalize().unwrap();
        let file = root.join("not accepted.zip");
        fs::write(&file, [0xff, 0xfe]).unwrap();
        let error = load_connected_file(Folder { path: root }, file.clone())
            .err()
            .unwrap();
        assert!(error.contains(file.to_str().unwrap()));
    }
    #[test]
    fn background_batch_is_staged_once_without_uploading_or_losing_draft() {
        let fixture = Fixture::new();
        let mut a = fixture.app();
        let path = fixture.0.join("Desktop Project/read me.rs");
        a.input.set("Existing draft");
        a.drop_in_background(format!(
            "'{}' '{}' and a question",
            path.display(),
            path.display()
        ))
        .unwrap();
        finish(&mut a);
        assert_eq!(a.attachments.len(), 1);
        assert_eq!(a.input.value(), "Existing draft and a question");
        assert!(a.pending.is_none());
        assert!(a.chats[a.active].messages.is_empty());
    }
    #[test]
    fn uncertain_drop_keeps_original_text_and_draft_without_sending() {
        let fixture = Fixture::new();
        let mut a = fixture.app();
        let text = format!(
            "'{}' explain this",
            fixture.0.join("missing file.txt").display()
        );
        a.input.set("Existing draft: ");
        a.drop_in_background(text.clone()).unwrap();
        finish(&mut a);
        assert_eq!(a.input.value(), format!("Existing draft: {text}"));
        assert_eq!(a.checked_drop, Some(a.input.value()));
        assert!(a.attachments.is_empty());
        assert!(a.pending.is_none());
        assert!(a.chats[a.active].messages.is_empty());
    }
    #[test]
    fn background_drop_stages_file_and_clears_previous_unmatched_hint() {
        let fixture = Fixture::new();
        let mut a = fixture.app();
        a.checked_drop = Some("old text".into());
        a.drop_in_background(format!(
            "'{}' explain this",
            fixture.0.join("Desktop Project/read me.rs").display()
        ))
        .unwrap();
        finish(&mut a);
        assert_eq!(a.attachments.len(), 1);
        assert_eq!(a.input.value(), "explain this");
        assert!(a.checked_drop.is_none());
        assert!(a.pending.is_none());
        assert!(a.chats[a.active].messages.is_empty());
    }
    #[test]
    fn abandoned_background_batch_never_stages_or_appends_text() {
        let fixture = Fixture::new();
        let mut a = fixture.app();
        let (tx, rx) = mpsc::channel();
        a.folder_rx = Some(rx);
        a.cancel_request();
        a.input.set("Keep this draft");
        let file = crate::attachments::load(&fixture.0.join("Desktop Project/read me.rs")).unwrap();
        tx.send(Ok(FolderResult::AttachedBatch(
            vec![file],
            Some("late text".into()),
        )))
        .unwrap();
        assert!(a.receive_folder());
        assert!(a.attachments.is_empty());
        assert_eq!(a.input.value(), "Keep this draft");
        assert!(a.pending.is_none());
    }
}
