//! FileCleaner CLI binary entrypoint.

use std::process;

fn main() {
    process::exit(filecleaner::run_from_env());
}
