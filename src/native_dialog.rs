use std::path::{Path, PathBuf};

use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

pub enum Confirm {
    Save,
    Discard,
    Cancel,
}

pub fn pick_open(title: &str, dir: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = FileDialog::new().set_title(title);
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_file()
}

pub fn pick_save(title: &str, dir: Option<&Path>, file_name: &str) -> Option<PathBuf> {
    let mut dialog = FileDialog::new()
        .set_title(title)
        .set_file_name(file_name);
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file()
}

pub fn unsaved(title: &str, body: &str, save: &str, dont_save: &str, cancel: &str) -> Confirm {
    let result = MessageDialog::new()
        .set_level(MessageLevel::Warning)
        .set_title(title)
        .set_description(body)
        .set_buttons(MessageButtons::YesNoCancelCustom(
            save.to_owned(),
            dont_save.to_owned(),
            cancel.to_owned(),
        ))
        .show();
    match result {
        MessageDialogResult::Yes => Confirm::Save,
        MessageDialogResult::No => Confirm::Discard,
        MessageDialogResult::Custom(label) if label == save => Confirm::Save,
        MessageDialogResult::Custom(label) if label == dont_save => Confirm::Discard,
        _ => Confirm::Cancel,
    }
}

pub fn error(title: &str, message: &str) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(title)
        .set_description(message)
        .set_buttons(MessageButtons::Ok)
        .show();
}
