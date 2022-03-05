use crate::util::OutputKind;
use indicatif::{ProgressBar, ProgressStyle};

// For more spinners check out the cli-spinners project:
// https://github.com/sindresorhus/cli-spinners/blob/master/spinners.json
pub const DOTS_12: &[&str; 56] = &[
    "⢀⠀", "⡀⠀", "⠄⠀", "⢂⠀", "⡂⠀", "⠅⠀", "⢃⠀", "⡃⠀", "⠍⠀", "⢋⠀", "⡋⠀", "⠍⠁", "⢋⠁", "⡋⠁", "⠍⠉", "⠋⠉",
    "⠋⠉", "⠉⠙", "⠉⠙", "⠉⠩", "⠈⢙", "⠈⡙", "⢈⠩", "⡀⢙", "⠄⡙", "⢂⠩", "⡂⢘", "⠅⡘", "⢃⠨", "⡃⢐", "⠍⡐", "⢋⠠",
    "⡋⢀", "⠍⡁", "⢋⠁", "⡋⠁", "⠍⠉", "⠋⠉", "⠋⠉", "⠉⠙", "⠉⠙", "⠉⠩", "⠈⢙", "⠈⡙", "⠈⠩", "⠀⢙", "⠀⡙", "⠀⠩",
    "⠀⢘", "⠀⡘", "⠀⠨", "⠀⢐", "⠀⡐", "⠀⠠", "⠀⢀", "⠀⡀",
];

pub struct Spinner {
    pub spinner: Option<ProgressBar>,
}

impl Spinner {
    pub(crate) fn new(output_kind: &OutputKind) -> Self {
        match output_kind {
            OutputKind::Text => {
                let style = ProgressStyle::default_spinner()
                    .tick_strings(DOTS_12)
                    .template("{spinner:.blue} {msg}");

                let spinner = ProgressBar::new_spinner().with_style(style);

                spinner.enable_steady_tick(80);
                Self {
                    spinner: Some(spinner),
                }
            }
            OutputKind::Json => Self { spinner: None },
        }
    }

    /// Handles updating the spinner for text output
    /// JSON output will be corrupted with a spinner
    pub fn update_spinner_message(&self, msg: String) {
        match &self.spinner {
            Some(spinner) => spinner.set_message(msg),
            None => {}
        }
    }

    pub fn finish_and_clear(&self) {
        match &self.spinner {
            Some(progress_bar) => progress_bar.finish_and_clear(),
            None => {}
        }
    }
}
