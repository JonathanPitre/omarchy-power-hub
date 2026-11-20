use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = include_str!("../bin/ops-priv");
const HOOK: &str = include_str!("../bin/sleep-hook");
const CLEANUP_USER: &str = include_str!("../bin/cleanup-user");
const UNIT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/extras/systemd");

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("ops-priv-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sys")).unwrap();
        let at = |d: &str| format!("{}{d}", root.display());
        let script = [("/sys/", "/sys/"), ("/usr/local/libexec/", "/libexec/"), ("/usr/lib/systemd/", "/systemd/"), ("/etc/", "/etc/")]
            .iter()
            .fold(SCRIPT.to_string(), |s, (from, to)| s.replace(from, &at(to)))
            .replace("PATH=/usr/bin", &format!("PATH={}/fakes:/usr/bin", root.display()));
        fs::write(root.join("ops-priv"), script).unwrap();
        Self(root)
    }

    fn put(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.0.join("sys").join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }

    fn usb(&self, name: &str, vendor: &str, classes: &[&str]) -> PathBuf {
        self.put(&format!("bus/usb/devices/{name}/idVendor"), &format!("{vendor}\n"));
        for (i, c) in classes.iter().enumerate() {
            self.put(&format!("bus/usb/devices/{name}/{name}:{i}.0/bInterfaceClass"), &format!("{c}\n"));
        }
        self.put(&format!("bus/usb/devices/{name}/power/wakeup"), "disabled\n")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new("bash").arg(self.0.join("ops-priv")).args(args).output().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap().trim().into()
}

#[test]
fn logitech_receiver_and_generic_hid_follow_the_mode_but_other_usb_is_left_alone() {
    let s = Sandbox::new("usb");
    let root_hub = s.usb("usb1", "1d6b", &[]);
    let receiver = s.usb("1-3", "046d", &["03", "03"]);
    let hid = s.usb("1-4", "1a2c", &["03"]);
    let other = s.usb("1-8", "0bda", &["ff"]);
    let out = s.run(&["hid-wakeup", "on"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
    assert_eq!([&receiver, &hid].map(|p| read(p)), ["enabled", "enabled"]);
    assert_eq!([&root_hub, &other].map(|p| read(p)), ["disabled", "disabled"]);
    assert!(s.run(&["hid-wakeup", "off"]).status.success());
    assert_eq!([&receiver, &hid].map(|p| read(p)), ["disabled", "disabled"]);
}

#[test]
fn uppercase_vendor_matches_and_device_without_interfaces_does_not_abort() {
    let s = Sandbox::new("vendor");
    let upper = s.usb("1-3", "046D", &[]);
    let bare = s.usb("1-9", "1d6b", &[]);
    assert!(s.run(&["hid-wakeup", "on"]).status.success());
    assert_eq!((read(&upper), read(&bare)), ("enabled".into(), "disabled".into()));
}

#[test]
fn device_without_wakeup_node_is_skipped() {
    let s = Sandbox::new("nowake");
    s.put("bus/usb/devices/1-3/idVendor", "046d\n");
    let out = s.run(&["hid-wakeup", "on"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
}

#[test]
fn i2c_devices_match_name_or_uppercase_folder() {
    let s = Sandbox::new("i2c");
    s.put("bus/i2c/devices/i2c-PIXA3854:00/name", "PIXA3854:00\n");
    let pad = s.put("bus/i2c/devices/i2c-PIXA3854:00/power/wakeup", "disabled\n");
    let upper = s.put("bus/i2c/devices/TOUCHPAD-1/power/wakeup", "disabled\n");
    let sensor = s.put("bus/i2c/devices/i2c-ACC0:00/power/wakeup", "disabled\n");
    s.put("bus/i2c/devices/i2c-ACC0:00/name", "ACC0:00\n");
    assert!(s.run(&["hid-wakeup", "on"]).status.success());
    assert_eq!((read(&pad), read(&upper), read(&sensor)), ("enabled".into(), "enabled".into(), "disabled".into()));
}

#[test]
fn unwritable_node_does_not_fail_the_run() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new("ro");
    let ro = s.usb("1-3", "046d", &[]);
    let ok = s.usb("1-4", "046d", &[]);
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o444)).unwrap();
    let out = s.run(&["hid-wakeup", "on"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
    assert_eq!(read(&ok), "enabled");
}

#[test]
fn bad_mode_exits_2() {
    let s = Sandbox::new("bad");
    assert_eq!(s.run(&["hid-wakeup", "maybe"]).status.code(), Some(2));
    assert_eq!(s.run(&["hid-wakeup"]).status.code(), Some(2));
}

#[test]
fn helper_install_records_the_owner_and_installs_the_sleep_hook() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new("install");
    let out = s.install();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let name = Command::new("id").arg("-nu").output().unwrap().stdout;
    let owner = s.at("libexec/omarchy-power-hub/owner");
    assert_eq!(fs::read(&owner).unwrap(), name);
    assert_eq!(fs::metadata(owner).unwrap().permissions().mode() & 0o777, 0o644);
    assert_eq!(fs::read_to_string(s.at("systemd/system-sleep/omarchy-power-hub")).unwrap(), HOOK);
}

#[test]
fn sleep_hook_exits_zero_without_an_owner() {
    let s = Sandbox::new("hook");
    let empty = s.0.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let script = s.0.join("hook");
    fs::write(&script, HOOK.replace("/usr/local/libexec/omarchy-power-hub", &empty.display().to_string())).unwrap();
    for arg in ["pre", "post"] {
        let out = Command::new("sh").arg(&script).arg(arg).output().unwrap();
        assert!(out.status.success() && out.stdout.is_empty() && out.stderr.is_empty(), "{arg}");
    }
}

#[test]
fn script_is_python_free_and_install_removes_the_old_file() {
    assert!(!SCRIPT.to_lowercase().contains("python"));
    assert!(SCRIPT.contains(r#"rm -f "$dir/hid_wakeup.py""#));
}

#[test]
fn epp_cpu_rejects_unsafe_arguments_before_sysfs_access() {
    let s = Sandbox::new("epp");
    for args in [["../etc", "power"].as_slice(), &["1x", "power"], &["0", "performance", "extra"]] {
        assert_eq!(s.run(&[&["epp-cpu"], args].concat()).status.code(), Some(2), "{args:?}");
    }
    let missing = s.run(&["epp-cpu", "999999", "power"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no cpu preference"));
}

impl Sandbox {
    fn fake(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let dir = self.0.join("fakes");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/bash\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Logging fakes for the commands the root verbs call; each call lands in `calls.log`.
    fn fake_commands(&self) {
        let log = self.0.join("calls.log");
        for name in ["systemctl", "logger", "hyprctl"] {
            self.fake(name, &format!(r#"echo "{name} $*" >> {}"#, log.display()));
        }
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.0.join("calls.log")).unwrap_or_default()
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.at(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }

    /// Lays out an installed helper for the current user, with the plugin folder absent.
    fn installed(&self, cleanup_user: &str) {
        self.fake_commands();
        fs::create_dir_all(self.home().join(".config/omarchy/plugins")).unwrap();
        let user = String::from_utf8(Command::new("id").arg("-nu").output().unwrap().stdout).unwrap();
        self.fake("getent", &format!(r#"echo "{}:x:1:1::{}:/bin/sh""#, user.trim(), self.home().display()));
        self.fake("runuser", r#"shift 3; exec "$@""#);
        self.write("libexec/omarchy-power-hub/owner", &user);
        for f in ["ops-priv", "hid_wakeup.py"] {
            self.write(&format!("libexec/omarchy-power-hub/{f}"), "x\n");
        }
        self.write("libexec/omarchy-power-hub/cleanup-user", cleanup_user);
        for f in [
            "etc/sudoers.d/omarchy-power-hub",
            "etc/sudoers.d/omarchy-power-hub-leds",
            "etc/sudoers.d/keep-me",
            "etc/systemd/sleep.conf.d/90-omarchy-power-hub.conf",
            "etc/systemd/system/omarchy-power-hub-cleanup.path",
            "etc/systemd/system/omarchy-power-hub-cleanup.service",
            "systemd/system-sleep/omarchy-power-hub",
            "systemd/system-sleep/battery-plus-sleep",
            "systemd/system-sleep/battery-plus-wifi-suspend-fix",
            "systemd/system-sleep/keyboard-backlight",
            "home/.config/omarchy/power-hub-tweaks.json",
            "home/.config/omarchy/omarchy-power-hub.json",
            "var-log/battery-plus-sleep.log",
        ] {
            self.write(f, "x\n");
        }
    }

    /// Runs helper-install as the current user from a plugin-like `bin/` next to `extras/`.
    fn install(&self) -> Output {
        let r = &self.0;
        self.fake_commands();
        self.fake("install", r#"a=(); while [ $# -gt 0 ]; do case "$1" in -o|-g) shift 2 ;; *) a+=("$1"); shift ;; esac; done; exec /usr/bin/install "${a[@]}""#);
        self.fake("visudo", "exit 0");
        let name = String::from_utf8(Command::new("id").arg("-nu").output().unwrap().stdout).unwrap();
        self.fake("getent", &format!(r#"echo "{}:x:1:1::{}:/bin/sh""#, name.trim(), self.home().display()));
        for d in ["systemd/system-sleep", "etc/sudoers.d", "etc/systemd/system", "bin"] {
            fs::create_dir_all(r.join(d)).unwrap();
        }
        fs::copy(r.join("ops-priv"), r.join("bin/ops-priv")).unwrap();
        self.write("bin/sleep-hook", HOOK);
        self.write("bin/cleanup-user", CLEANUP_USER);
        for u in ["path", "service"] {
            self.write(&format!("extras/systemd/omarchy-power-hub-cleanup.{u}"), &fs::read_to_string(format!("{UNIT_DIR}/omarchy-power-hub-cleanup.{u}")).unwrap());
        }
        let uid = Command::new("id").arg("-u").output().unwrap().stdout;
        Command::new("bash").arg(r.join("bin/ops-priv")).arg("helper-install").env("PKEXEC_UID", String::from_utf8_lossy(&uid).trim()).output().unwrap()
    }

    fn cleanup(&self) -> Output {
        let out = self.run(&["cleanup", "--anything", "x"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
        out
    }

    fn cleanup_user(&self) -> Output {
        Command::new("sh")
            .arg(self.at("cleanup-user"))
            .env("HOME", self.home())
            .env("PATH", format!("{}:/usr/bin:/bin", self.at("fakes").display()))
            .output()
            .unwrap()
    }
}

#[test]
fn cleanup_removes_nothing_while_the_plugin_folder_exists() {
    let s = Sandbox::new("cleanup-present");
    s.installed("touch \"$HOME/ran\"\n");
    fs::create_dir_all(s.home().join(".config/omarchy/plugins/omarchy-power-hub")).unwrap();
    s.cleanup();
    assert!(s.at("libexec/omarchy-power-hub/owner").exists() && s.at("etc/sudoers.d/omarchy-power-hub").exists());
    assert_eq!(s.calls(), "");
    assert!(!s.home().join("ran").exists());
}

#[test]
fn cleanup_treats_a_broken_symlink_as_present() {
    let s = Sandbox::new("cleanup-link");
    s.installed("touch \"$HOME/ran\"\n");
    let plugins = s.home().join(".config/omarchy/plugins");
    fs::create_dir_all(&plugins).unwrap();
    std::os::unix::fs::symlink(s.at("nowhere"), plugins.join("omarchy-power-hub")).unwrap();
    s.cleanup();
    assert!(s.at("libexec/omarchy-power-hub/owner").exists() && s.at("etc/sudoers.d/omarchy-power-hub").exists());
    assert!(!s.home().join("ran").exists());
}

#[test]
fn cleanup_exits_ok_without_an_owner() {
    let s = Sandbox::new("cleanup-noowner");
    s.fake_commands();
    s.cleanup();
    assert_eq!(s.calls(), "");
}

#[test]
fn cleanup_removes_every_root_file_and_keeps_user_config() {
    let s = Sandbox::new("cleanup-all");
    s.installed("touch \"$HOME/ran\"\n");
    s.cleanup();
    assert!(s.home().join("ran").exists());
    for gone in [
        "libexec/omarchy-power-hub",
        "etc/sudoers.d/omarchy-power-hub",
        "etc/sudoers.d/omarchy-power-hub-leds",
        "etc/systemd/sleep.conf.d/90-omarchy-power-hub.conf",
        "etc/systemd/system/omarchy-power-hub-cleanup.path",
        "etc/systemd/system/omarchy-power-hub-cleanup.service",
        "systemd/system-sleep/omarchy-power-hub",
        "systemd/system-sleep/battery-plus-sleep",
        "systemd/system-sleep/battery-plus-wifi-suspend-fix",
    ] {
        assert!(!s.at(gone).exists(), "{gone}");
    }
    for kept in [
        "home/.config/omarchy/power-hub-tweaks.json",
        "home/.config/omarchy/omarchy-power-hub.json",
        "var-log/battery-plus-sleep.log",
        "etc/sudoers.d/keep-me",
        "systemd/system-sleep/keyboard-backlight",
    ] {
        assert!(s.at(kept).exists(), "{kept}");
    }
    let calls = s.calls();
    let at = |needle: &str| calls.find(needle).unwrap_or_else(|| panic!("{needle} in {calls}"));
    assert!(at("systemctl disable --now omarchy-power-hub-cleanup.path") < at("systemctl daemon-reload"));
}

#[test]
fn cleanup_continues_when_cleanup_user_fails() {
    let s = Sandbox::new("cleanup-fails");
    s.installed("exit 1\n");
    s.cleanup();
    assert!(!s.at("etc/sudoers.d/omarchy-power-hub").exists() && !s.at("libexec/omarchy-power-hub").exists());
    assert!(s.calls().contains("logger -t omarchy-power-hub"));
}

const LUA: &str = "before\n-- BEGIN Omarchy Power Hub lid action override\nhl.unbind(1)\n-- END Omarchy Power Hub lid action override\nafter\n";

#[test]
fn cleanup_user_removes_only_the_marked_lid_block() {
    let s = Sandbox::new("user-block");
    s.fake_commands();
    s.write("cleanup-user", CLEANUP_USER);
    let lua = s.write("home/.config/hypr/bindings.lua", LUA);
    let unit = s.write("home/.config/systemd/user/omarchy-power-hub-tweaks.service", "x\n");
    let wants = s.write("home/.config/systemd/user/default.target.wants/omarchy-power-hub-tweaks.service", "x\n");
    assert!(s.cleanup_user().status.success());
    assert_eq!(fs::read_to_string(lua).unwrap(), "before\nafter\n");
    assert!(!unit.exists() && !wants.exists());
    let calls = s.calls();
    assert!(calls.contains("systemctl --user disable omarchy-power-hub-tweaks.service"));
    assert!(calls.contains("hyprctl -i 0 reload"));
}

#[test]
fn cleanup_user_leaves_bindings_alone_without_both_markers() {
    let s = Sandbox::new("user-markers");
    s.fake_commands();
    s.write("cleanup-user", CLEANUP_USER);
    let half = LUA.replace("-- END Omarchy Power Hub lid action override\n", "");
    let lua = s.write("home/.config/hypr/bindings.lua", &half);
    assert!(s.cleanup_user().status.success());
    assert_eq!(fs::read_to_string(lua).unwrap(), half);
}

#[test]
fn helper_install_installs_and_enables_the_cleanup_units() {
    let s = Sandbox::new("units");
    assert!(s.install().status.success());
    let path = fs::read_to_string(s.at("etc/systemd/system/omarchy-power-hub-cleanup.path")).unwrap();
    assert!(path.contains(&format!("PathChanged={}/.config/omarchy/plugins\n", s.home().display())), "{path}");
    assert!(!path.contains("@HOME@"));
    let service = fs::read_to_string(s.at("etc/systemd/system/omarchy-power-hub-cleanup.service")).unwrap();
    assert!(service.contains("ExecStart=/usr/local/libexec/omarchy-power-hub/ops-priv cleanup"));
    assert_eq!(fs::read_to_string(s.at("libexec/omarchy-power-hub/cleanup-user")).unwrap(), CLEANUP_USER);
    assert_eq!(s.calls(), "systemctl daemon-reload\nsystemctl enable --now omarchy-power-hub-cleanup.path\n");
}

#[test]
fn cleanup_user_keeps_the_rest_when_end_comes_before_begin() {
    let s = Sandbox::new("user-order");
    s.fake_commands();
    s.write("cleanup-user", CLEANUP_USER);
    let text = "a\n-- END Omarchy Power Hub lid action override\nb\n-- BEGIN Omarchy Power Hub lid action override\nc\n";
    let lua = s.write("home/.config/hypr/bindings.lua", text);
    assert!(s.cleanup_user().status.success());
    assert_eq!(fs::read_to_string(lua).unwrap(), text);
}

#[test]
fn cleanup_removes_nothing_when_the_plugins_folder_is_unreadable() {
    let s = Sandbox::new("cleanup-noplugins");
    s.installed("touch \"$HOME/ran\"\n");
    fs::remove_dir(s.home().join(".config/omarchy/plugins")).unwrap();
    s.cleanup();
    assert!(s.at("libexec/omarchy-power-hub/owner").exists() && s.at("etc/sudoers.d/omarchy-power-hub").exists());
    assert!(!s.home().join("ran").exists());
}
