//! The groups a dashboard scan is split into, with the plain-language facts
//! the overview and the review list show for each.

/// How careful the user should be before cleaning a category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Safety {
    /// Rebuilt automatically by the system or the app that made it.
    Safe,
    /// May hold something the user wants (logins, own folders).
    ReviewFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScanCategory {
    System,
    Applications,
    /// Download and build caches of programming tools (cargo, npm, pip…).
    DevTools,
    Logs,
    Browsers,
    Custom,
    /// Build and dependency folders of projects nobody has touched lately.
    Projects,
}

impl ScanCategory {
    pub const ALL: [ScanCategory; 7] = [
        Self::System,
        Self::Applications,
        Self::DevTools,
        Self::Logs,
        Self::Browsers,
        Self::Projects,
        Self::Custom,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::System => "System Junk",
            Self::Applications => "App Caches",
            Self::DevTools => "Developer Tools",
            Self::Logs => "Application Logs",
            Self::Browsers => "Browser Data",
            Self::Custom => "Custom Directories",
            Self::Projects => "Inactive Projects",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::System => "Temporary files, thumbnails, crash reports, package caches and the trash.",
            Self::Applications => "Caches that apps rebuild on their own the next time they need them.",
            Self::DevTools => "Package and build caches of tools such as cargo, npm, pip and Gradle. They are downloaded again when a project needs them.",
            Self::Logs => "Old log files kept past the retention period you set.",
            Self::Browsers => "Browser caches, and the cookies, history or sessions you chose. Cookies sign you out of websites.",
            Self::Custom => "Folders you added yourself. Check that nothing you need is inside.",
            Self::Projects => "Dependency and build folders (node_modules, target, .venv…) of projects you have not changed in a while. Building the project again brings them back.",
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::System => "sidebar/user-system",
            Self::Applications => "sidebar/applications",
            Self::DevTools => "pc-utilities-terminal-symbolic",
            Self::Logs => "pc-text-x-generic-symbolic",
            Self::Browsers => "sidebar/browsers",
            Self::Custom => "sidebar/custom-directories",
            Self::Projects => "pc-folder-projects-symbolic",
        }
    }

    pub fn safety(self) -> Safety {
        match self {
            Self::System | Self::Applications | Self::DevTools | Self::Logs => Safety::Safe,
            Self::Browsers | Self::Custom | Self::Projects => Safety::ReviewFirst,
        }
    }

    /// Bar and dot colour (GNOME palette, readable on light and dark).
    pub fn color(self) -> (f64, f64, f64) {
        let hex = match self {
            Self::System => 0x3584e4,
            Self::Applications => 0x2ec27e,
            Self::DevTools => 0xe5a50a,
            Self::Logs => 0x9141ac,
            Self::Browsers => 0xe66100,
            Self::Custom => 0xc64600,
            Self::Projects => 0x865e3c,
        };
        (
            ((hex >> 16) & 0xff) as f64 / 255.0,
            ((hex >> 8) & 0xff) as f64 / 255.0,
            (hex & 0xff) as f64 / 255.0,
        )
    }

    /// Sidebar page that holds this category's rules, if any.
    pub fn page_name(self) -> Option<&'static str> {
        match self {
            Self::System => Some("system"),
            Self::Applications | Self::DevTools => Some("applications"),
            Self::Browsers => Some("browsers"),
            Self::Custom => Some("custom"),
            Self::Logs | Self::Projects => Some("settings"),
        }
    }
}
