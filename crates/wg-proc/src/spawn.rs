//! The one place that creates child processes.
//!
//! Every spawn in the workspace goes through [`Spawn`]. The invariant is that no spawn ever
//! creates a new visible console window, on Windows:
//!
//! * a process that already has a console passes it on: the child inherits that console and no
//!   console flag is added, so the child costs nothing extra;
//! * a process with no console (a detached or windowless server, say) gets `CREATE_NO_WINDOW`,
//!   so the child gets a hidden console instead of a visible one;
//! * [`Spawn::force_hidden_console`] always adds `CREATE_NO_WINDOW`, whatever the launcher has,
//!   for a process that must own one hidden console for its own children to inherit (the daemon);
//! * `DETACHED_PROCESS` is never set.
//!
//! Direct use of `std::process::Command::new` and `CommandExt::creation_flags` is a clippy error
//! (`clippy.toml`, `disallowed-methods`), so this module is the only exception.

use std::ffi::OsStr;
use std::ops::{Deref, DerefMut};
use std::process::Command;

/// `CREATE_NO_WINDOW`: the child has a console, but no window is shown for it.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_PROCESS_GROUP`: the child leads its own process group.
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// `DETACHED_PROCESS`: the child has no console at all. Never used here: it leaves every console
/// child of the process without a console of its own, which opens a window for each of them.
pub const DETACHED_PROCESS: u32 = 0x0000_0008;

/// The creation flags a child gets on Windows.
///
/// `requested` is what the caller asked for; `DETACHED_PROCESS` is always stripped from it.
/// `console_present` says whether this process already has a console (see [`console_present`]),
/// and `force_hidden` whether the caller needs a hidden console of its own regardless. A child
/// gets `CREATE_NO_WINDOW` when the launcher has no console or `force_hidden` is set, and no
/// console flag otherwise, so it inherits the launcher's console.
#[cfg_attr(not(windows), allow(dead_code))]
#[must_use]
pub fn creation_flags_for(requested: u32, console_present: bool, force_hidden: bool) -> u32 {
    let flags = requested & !DETACHED_PROCESS;
    if force_hidden || !console_present {
        flags | CREATE_NO_WINDOW
    } else {
        flags
    }
}

/// Whether this process is attached to a console, windowless ones included.
///
/// `GetConsoleCP` is nonzero for any attached console, including one created with
/// `CREATE_NO_WINDOW`, for which `GetConsoleWindow` returns null and so cannot be used.
#[cfg(windows)]
#[must_use]
pub fn console_present() -> bool {
    // SAFETY: `GetConsoleCP` takes no arguments and only reads the calling process's console state.
    unsafe { windows_sys::Win32::System::Console::GetConsoleCP() != 0 }
}

/// A child process being prepared. It derefs to [`std::process::Command`], so arguments, stdio,
/// environment and the working directory are set as usual, and it is spawned with `spawn`,
/// `status` or `output`.
pub struct Spawn {
    command: Command,
    #[cfg_attr(not(windows), allow(dead_code))]
    flags: u32,
    #[cfg_attr(not(windows), allow(dead_code))]
    force_hidden: bool,
}

impl Spawn {
    /// Prepare `program`, with the console rules above applied on Windows.
    #[must_use]
    #[allow(
        clippy::disallowed_methods,
        reason = "the chokepoint: the only production call to Command::new"
    )]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        let mut spawn = Self {
            command: Command::new(program),
            flags: 0,
            force_hidden: false,
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

    /// Give the child a hidden console of its own (`CREATE_NO_WINDOW`) even when this process has
    /// a console, so that the children it starts inherit that one hidden console. Used by the
    /// daemon. On POSIX this changes nothing.
    pub fn force_hidden_console(&mut self) -> &mut Self {
        self.force_hidden = true;
        self.apply_flags();
        self
    }

    #[cfg(windows)]
    #[allow(
        clippy::disallowed_methods,
        reason = "the chokepoint: the only production call to creation_flags, always with the effective flags"
    )]
    fn apply_flags(&mut self) {
        use std::os::windows::process::CommandExt;
        let flags = creation_flags_for(self.flags, console_present(), self.force_hidden);
        self.command.creation_flags(flags);
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
    use super::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS, creation_flags_for};

    #[test]
    fn child_of_a_process_with_a_console_adds_no_console_flag() {
        assert_eq!(creation_flags_for(0, true, false), 0);
        assert_eq!(
            creation_flags_for(CREATE_NEW_PROCESS_GROUP, true, false),
            CREATE_NEW_PROCESS_GROUP
        );
    }

    #[test]
    fn child_of_a_process_without_a_console_gets_a_hidden_one() {
        assert_eq!(creation_flags_for(0, false, false), CREATE_NO_WINDOW);
        assert_eq!(
            creation_flags_for(CREATE_NEW_PROCESS_GROUP, false, false),
            CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        );
    }

    #[test]
    fn daemon_gets_a_hidden_console_and_its_group_even_with_a_console() {
        assert_eq!(
            creation_flags_for(CREATE_NEW_PROCESS_GROUP, true, true),
            CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        );
        assert_eq!(
            creation_flags_for(CREATE_NEW_PROCESS_GROUP, false, true),
            CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        );
    }

    #[test]
    fn detached_is_stripped_in_every_case() {
        let requested = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        for console in [true, false] {
            for force in [true, false] {
                let flags = creation_flags_for(requested, console, force);
                assert_eq!(
                    flags & DETACHED_PROCESS,
                    0,
                    "console={console} force={force}"
                );
                assert_eq!(flags & CREATE_NEW_PROCESS_GROUP, CREATE_NEW_PROCESS_GROUP);
            }
        }
    }
}
