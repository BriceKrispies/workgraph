//! The one place that creates child processes.
//!
//! Every spawn in the workspace goes through [`Spawn`]. On Windows it always sets
//! `CREATE_NO_WINDOW`, so a child never gets a console of its own: a console program started
//! without a console (a detached daemon, say) would otherwise open a visible terminal window
//! for every console child it starts. Callers may add flags, such as the process group, but
//! cannot clear `CREATE_NO_WINDOW`, and `DETACHED_PROCESS` is always stripped. Direct use of
//! `std::process::Command::new` is a clippy error (`clippy.toml`, `disallowed-methods`), so this
//! module is the only exception.

use std::ffi::OsStr;
use std::ops::{Deref, DerefMut};
use std::process::Command;

/// `CREATE_NO_WINDOW`: the child has a console, but no window is shown for it.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_PROCESS_GROUP`: the child leads its own process group.
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// `DETACHED_PROCESS`: the child has no console at all. Never used here: it gives every console
/// child of the process a new console, which is the window flood this module prevents.
pub const DETACHED_PROCESS: u32 = 0x0000_0008;

/// The creation flags a child really gets on Windows, given the flags a caller asked for.
/// `CREATE_NO_WINDOW` is always set and `DETACHED_PROCESS` is always cleared; everything else the
/// caller asked for is kept.
#[cfg_attr(not(windows), allow(dead_code))]
#[must_use]
pub fn effective_creation_flags(requested: u32) -> u32 {
    (requested & !DETACHED_PROCESS) | CREATE_NO_WINDOW
}

/// A child process being prepared. It derefs to [`std::process::Command`], so arguments, stdio,
/// environment and the working directory are set as usual, and it is spawned with `spawn`,
/// `status` or `output`.
pub struct Spawn {
    command: Command,
    #[cfg_attr(not(windows), allow(dead_code))]
    flags: u32,
}

impl Spawn {
    /// Prepare `program`, with the console hidden on Windows.
    #[must_use]
    #[allow(
        clippy::disallowed_methods,
        reason = "the chokepoint: the only production call to Command::new"
    )]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        let mut spawn = Self {
            command: Command::new(program),
            flags: 0,
        };
        spawn.apply_flags();
        spawn
    }

    /// Put the child in a process group of its own: on POSIX its own group, on Windows
    /// `CREATE_NEW_PROCESS_GROUP`. A console interrupt aimed at the caller is then not delivered
    /// to the child, and the child's tree can be addressed as one unit.
    pub fn own_process_group(&mut self) -> &mut Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            self.command.process_group(0);
        }
        #[cfg(windows)]
        {
            self.flags |= CREATE_NEW_PROCESS_GROUP;
            self.apply_flags();
        }
        self
    }

    #[cfg(windows)]
    #[allow(
        clippy::disallowed_methods,
        reason = "the chokepoint: the only production call to creation_flags, always with the effective flags"
    )]
    fn apply_flags(&mut self) {
        use std::os::windows::process::CommandExt;
        self.command
            .creation_flags(effective_creation_flags(self.flags));
    }

    #[cfg(not(windows))]
    fn apply_flags(&mut self) {}
}

impl Deref for Spawn {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.command
    }
}

impl DerefMut for Spawn {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS, effective_creation_flags,
    };

    #[test]
    fn plain_child_gets_only_no_window() {
        assert_eq!(effective_creation_flags(0), CREATE_NO_WINDOW);
    }

    #[test]
    fn process_group_child_keeps_its_group_and_no_window() {
        assert_eq!(
            effective_creation_flags(CREATE_NEW_PROCESS_GROUP),
            CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        );
    }

    #[test]
    fn daemon_request_for_detached_loses_detached_and_keeps_group() {
        let requested = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        assert_eq!(
            effective_creation_flags(requested),
            CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        );
    }
}
