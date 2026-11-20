// Adapted from Purge ExplanationDatabase.swift and Resources/explanations.json.
// MIT, © 2026 Jithin Sabu — https://github.com/jithin-sabu/purge-app.
// See THIRD_PARTY_LICENSES.md. Linux-only entries and policies are added below.
use super::Safety;

pub struct Explanation {
    pub key: &'static str,
    pub title: &'static str,
    pub text: &'static str,
    pub safety: Safety,
    pub aliases: &'static [&'static str],
}

pub const EXPLANATIONS: &[Explanation] = &[
    Explanation { key: "paru", title: "Paru Cache", text: "AUR sources and build files downloaded by Paru. They are downloaded and rebuilt when needed.", safety: Safety::Safe, aliases: &["paru"] },
    Explanation { key: "yay", title: "Yay Cache", text: "AUR sources and build files downloaded by Yay. They are downloaded and rebuilt when needed.", safety: Safety::Safe, aliases: &["yay"] },
    Explanation { key: "vscode", title: "VS Code Cache", text: "Temporary files stored by Visual Studio Code. Safe to delete and will be recreated when you open VS Code again.", safety: Safety::Safe, aliases: &["Code", "CachedData", "CachedExtensionVSIXs"] },
    Explanation { key: "cursor", title: "Cursor Cache", text: "Temporary files stored by the Cursor code editor. Safe to delete and will be recreated when you open Cursor again.", safety: Safety::Safe, aliases: &["Cursor"] },
    Explanation { key: "jetbrains", title: "JetBrains IDE Cache", text: "Indexes and caches JetBrains IDEs rebuild on their own. Local History is left alone. Each project re-indexes when you next open it, which takes a few minutes.", safety: Safety::Safe, aliases: &["JetBrains", "JetPackCache"] },
    Explanation { key: "zed", title: "Zed Editor Cache", text: "Temporary files stored by the Zed code editor. Safe to delete and will be recreated when you open Zed again.", safety: Safety::Safe, aliases: &["zed", "Zed"] },
    Explanation { key: "go", title: "Go Module Cache", text: "Cached Go packages and build outputs. Safe to delete and will be re-downloaded when you build your Go projects again.", safety: Safety::Safe, aliases: &["go-build"] },
    Explanation { key: "maven", title: "Maven Package Cache", text: "Maven's downloaded libraries and your own mvn install builds. Libraries download again on your next build; your own builds come back only if you rebuild them.", safety: Safety::ReviewFirst, aliases: &["maven", ".m2"] },
    Explanation { key: "composer", title: "Composer Cache", text: "Cached PHP packages downloaded by Composer. Safe to delete and will be re-downloaded when you run composer install.", safety: Safety::Safe, aliases: &["composer", ".composer"] },
    Explanation { key: "cargo", title: "Rust Package Cache", text: "Cached Rust packages and compiled dependencies. Safe to delete and will be re-downloaded when you build your Rust projects.", safety: Safety::Safe, aliases: &["cargo", ".cargo"] },
    Explanation { key: "electron", title: "Electron App Cache", text: "Temporary files stored by apps built with Electron like Slack, Discord, and Notion. Safe to delete and will be recreated automatically.", safety: Safety::Safe, aliases: &["electron", "Code Cache"] },
    Explanation { key: "bun-cache", title: "Bun Package Cache", text: "Packages downloaded by Bun, a JavaScript runtime and package manager. Safe to delete; they download again the next time you run bun install.", safety: Safety::Safe, aliases: &[".bun", "bun-install"] },
    Explanation { key: "deno-cache", title: "Deno Module Cache", text: "Cached modules downloaded by Deno, a JavaScript and TypeScript runtime. Safe to delete and will be re-downloaded when you next run a Deno project.", safety: Safety::Safe, aliases: &["deno"] },
    Explanation { key: "uv-cache", title: "uv Package Cache", text: "Python packages uv downloaded and built. uv gets them again when needed, but tools running through uvx can stop, and environments in symlink mode break.", safety: Safety::ReviewFirst, aliases: &["uv", "uv-cache", ".uv"] },
    Explanation { key: "gradle-global-cache", title: "Gradle Dependency Cache", text: "Gradle's downloaded libraries, build cache and wrapper versions. Nothing is lost, but the next build downloads and rebuilds them, which is slow.", safety: Safety::ReviewFirst, aliases: &["Gradle Dependency Cache"] },
    Explanation { key: "pub-cache", title: "Dart Pub Cache", text: "Dart and Flutter packages downloaded by pub; they download again on your next flutter pub get. Tools from dart pub global activate are left alone.", safety: Safety::Safe, aliases: &["pub-cache", ".pub-cache"] },
    Explanation { key: "pnpm-store", title: "pnpm Package Store", text: "The central package store used by pnpm to save disk space across projects. Safe to delete and packages will be re-downloaded when you run pnpm install.", safety: Safety::Safe, aliases: &["pnpm-store", ".pnpm-store"] },
    Explanation { key: "yarn-cache", title: "Yarn Package Cache", text: "Cached packages downloaded by Yarn. Safe to delete and will be re-downloaded when you run yarn install.", safety: Safety::Safe, aliases: &["yarn-cache", "yarn", "berry"] },
    Explanation { key: "npm-cache", title: "npm Package Cache", text: "Cached packages downloaded by npm. Safe to delete and will be re-downloaded when you run npm install.", safety: Safety::Safe, aliases: &["npm-cache", "npm"] },
    Explanation { key: "npm-npx-cache", title: "npm npx Cache", text: "Temporary packages downloaded when you run npx. Safe to delete; npx will download them again when needed.", safety: Safety::Safe, aliases: &["npm-npx-cache", "_npx"] },
    Explanation { key: "npm-logs", title: "npm Logs", text: "Log files written by npm commands. Safe to delete with no effect on installed packages.", safety: Safety::Safe, aliases: &["npm-logs", "_logs"] },
    Explanation { key: "corepack-cache", title: "Corepack Cache", text: "Cached Node.js package manager shims managed by Corepack. Safe to delete; Corepack will restore them on the next use.", safety: Safety::Safe, aliases: &["corepack-cache", "corepack"] },
    Explanation { key: "old-cli-versions", title: "Old AI Coding Tool Versions", text: "Earlier versions of Claude Code, Cursor Agent or Grok that stayed behind after updates. The version you run and the newest one are kept. Safe to delete.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "pre-commit-cache", title: "pre-commit Environments", text: "Tool environments that pre-commit sets up to run your git hooks. Safe to delete; they are set up again on your next commit, which makes that commit slow.", safety: Safety::Safe, aliases: &["pre-commit"] },
    Explanation { key: "pip", title: "pip Cache", text: "Downloaded Python packages are downloaded again when pip needs them.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "node-gyp", title: "node-gyp Headers", text: "Node.js headers used to build native add-ons are downloaded again on the next native build.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "thumbnails", title: "Thumbnails", text: "Image previews are recreated when you browse your files.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "updater", title: "App Update Downloads", text: "Downloaded app update packages are left behind after installation and can be downloaded again.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "browser", title: "Browser Cache", text: "Temporary website data is downloaded again; the next visit may load more slowly.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "unknown", title: "Application Cache", text: "This cache is not classified; check its contents before removing it.", safety: Safety::ReviewFirst, aliases: &[] },
    Explanation { key: "projects", title: "Inactive Rust Build Files", text: "Build outputs from inactive Rust projects are recreated on the next build.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "packages", title: "Package Manager Cache", text: "Downloaded packages can be downloaded again; cleaning requires administrator approval.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "journal", title: "System Journal", text: "Total journal storage, including active logs; administrator approval removes only archived logs older than your retention period.", safety: Safety::ReviewFirst, aliases: &[] },
    Explanation { key: "flatpak", title: "Unused Flatpak Runtimes", text: "Runtimes no installed app needs are removed by Flatpak; system runtimes require administrator approval.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "kernels", title: "Old Kernels", text: "Review obsolete kernel packages before removing them with administrator approval; the running and newest kernels are kept.", safety: Safety::ReviewFirst, aliases: &[] },
    Explanation { key: "docker", title: "Docker Leftovers", text: "Remove stopped containers, unused networks, dangling images and build cache; running containers and volumes are kept.", safety: Safety::ReviewFirst, aliases: &[] },
    Explanation { key: "chrome", title: "Google Chrome", text: "Google Chrome's temporary browsing data, such as website caches and previews. Websites may load a little slower until Chrome rebuilds it.", safety: Safety::Safe, aliases: &["com.google.Chrome", "Google", "Chrome"] },
    Explanation { key: "firefox", title: "Firefox Browser", text: "Temporary browsing data stored by Firefox. Deleting it is safe but websites may load a little slower until Firefox rebuilds the cache.", safety: Safety::Safe, aliases: &["org.mozilla.firefox", "Firefox"] },
    Explanation { key: "brave", title: "Brave Browser", text: "Temporary browsing data stored by the Brave browser. Deleting it is safe but websites may load a little slower until Brave rebuilds the cache.", safety: Safety::Safe, aliases: &["com.brave.Browser", "Brave"] },
    Explanation { key: "slack", title: "Slack", text: "Temporary files stored by Slack including message thumbnails and previews. Safe to delete and Slack will rebuild them automatically.", safety: Safety::Safe, aliases: &["com.tinyspeck.slackmacgap", "Slack"] },
    Explanation { key: "discord", title: "Discord", text: "Temporary files stored by Discord. Safe to delete and Discord will rebuild them automatically when you next open it.", safety: Safety::Safe, aliases: &["com.hnc.Discord", "Discord"] },
    Explanation { key: "zoom", title: "Zoom", text: "Temporary files stored by Zoom. Safe to delete and Zoom will recreate them automatically.", safety: Safety::Safe, aliases: &["us.zoom.xos", "Zoom"] },
    Explanation { key: "spotify", title: "Spotify", text: "Cached songs and artwork may include offline downloads; check before removing them.", safety: Safety::ReviewFirst, aliases: &["com.spotify.client", "Spotify"] },
    Explanation { key: "microsoft-teams", title: "Microsoft Teams", text: "Temporary files stored by Microsoft Teams. Safe to delete and Teams will rebuild them when you next open it.", safety: Safety::Safe, aliases: &["com.microsoft.teams2", "Teams", "Microsoft Teams"] },
    Explanation { key: "generic-cache", title: "Application Cache", text: "This is a known cache folder that apps or developer tools recreate automatically.", safety: Safety::Safe, aliases: &[] },
    Explanation { key: "model-cache", title: "AI Model Cache", text: "Downloaded AI models can be large and needed offline; check before downloading them again.", safety: Safety::ReviewFirst, aliases: &[] },
    Explanation { key: "keyring-cache", title: "Keyring Cache", text: "This folder may contain credentials or personal keyring data; review its contents before cleaning.", safety: Safety::ReviewFirst, aliases: &[] },
    Explanation { key: "session-cache", title: "Session Database Cache", text: "This folder contains session or login databases; cleaning it may sign you out or remove session data.", safety: Safety::ReviewFirst, aliases: &[] },
    Explanation { key: "orphans", title: "Orphaned Pacman Packages", text: "Dependencies no installed package needs may still be useful to you; review before removing them with administrator approval.", safety: Safety::ReviewFirst, aliases: &[] },

 ];

pub fn explanation(key: &str) -> &'static Explanation {
    EXPLANATIONS
        .iter()
        .find(|entry| {
            entry.key.eq_ignore_ascii_case(key)
                || entry
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(key))
        })
        .unwrap_or_else(|| {
            EXPLANATIONS
                .iter()
                .find(|entry| entry.key == "unknown")
                .unwrap()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_and_user_build_caches_require_review() {
        assert_eq!(explanation("unrecognised-app").safety, Safety::ReviewFirst);
        assert_eq!(explanation("maven").safety, Safety::ReviewFirst);
        assert_eq!(explanation("uv").safety, Safety::ReviewFirst);
        assert_eq!(explanation("docker").safety, Safety::ReviewFirst);
        assert_eq!(explanation("kernels").safety, Safety::ReviewFirst);
        assert_eq!(explanation("NODE-GYP").safety, Safety::Safe);
        assert_eq!(explanation("flatpak").safety, Safety::Safe);
        assert_eq!(explanation("orphans").safety, Safety::ReviewFirst);
        assert_eq!(explanation("CODE").key, "vscode");
    }
}
