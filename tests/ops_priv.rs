use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = include_str!("../bin/ops-priv");

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("ops-priv-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sys")).unwrap();
        let fake = format!("{}/sys/", root.display());
        fs::write(root.join("ops-priv"), SCRIPT.replace("/sys/", &fake)).unwrap();
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
