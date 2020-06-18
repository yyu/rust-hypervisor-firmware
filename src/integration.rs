// Copyright © 2020 Intel Corporation
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Integration tests live in this file. We can't use the Rust "integration test" mode
// as we don't have the expected source code structure.

#[cfg(feature = "integration_tests")]
#[cfg(test)]
mod tests {
    use rand::Rng;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::process::{Child, Command};
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::thread;
    use tempdir::TempDir;

    static COUNTER: AtomicUsize = AtomicUsize::new(6);

    struct GuestNetworkConfig {
        guest_ip: String,
        host_ip: String,
        guest_mac: String,
        tap_name: String,
    }

    impl GuestNetworkConfig {
        fn new(counter: u8) -> Self {
            // Generate a fully random MAC
            let mut m = rand::thread_rng().gen::<[u8; 6]>();

            // Set the first byte to make the OUI a locally administered OUI
            m[0] = 0x2e;

            let guest_mac = format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            );

            Self {
                guest_mac,
                host_ip: format!("192.168.{}.1", counter),
                guest_ip: format!("192.168.{}.2", counter),
                tap_name: format!("fwtap{}", counter),
            }
        }
    }

    trait CloudInit {
        fn prepare(&self, tmp_dir: &TempDir, network: &GuestNetworkConfig) -> String;
    }

    struct ClearCloudInit {}
    impl CloudInit for ClearCloudInit {
        fn prepare(&self, tmp_dir: &TempDir, network: &GuestNetworkConfig) -> String {
            let cloudinit_file_path =
                String::from(tmp_dir.path().join("cloudinit").to_str().unwrap());
            let cloud_init_directory = tmp_dir
                .path()
                .join("cloud-init")
                .join("clear")
                .join("openstack");
            fs::create_dir_all(&cloud_init_directory.join("latest"))
                .expect("Expect creating cloud-init directory to succeed");
            let source_file_dir = std::env::current_dir()
                .unwrap()
                .join("resources")
                .join("cloud-init")
                .join("clear")
                .join("openstack")
                .join("latest");
            fs::copy(
                source_file_dir.join("meta_data.json"),
                cloud_init_directory.join("latest").join("meta_data.json"),
            )
            .expect("Expect copying cloud-init meta_data.json to succeed");
            let mut user_data_string = String::new();
            fs::File::open(source_file_dir.join("user_data"))
                .unwrap()
                .read_to_string(&mut user_data_string)
                .expect("Expected reading user_data file in to succeed");
            user_data_string = user_data_string.replace("192.168.2.1", &network.host_ip);
            user_data_string = user_data_string.replace("192.168.2.2", &network.guest_ip);

            user_data_string = user_data_string.replace("12:34:56:78:90:ab", &network.guest_mac);
            fs::File::create(cloud_init_directory.join("latest").join("user_data"))
                .unwrap()
                .write_all(&user_data_string.as_bytes())
                .expect("Expected writing out user_data to succeed");
            std::process::Command::new("mkdosfs")
                .args(&["-n", "config-2"])
                .args(&["-C", cloudinit_file_path.as_str()])
                .arg("8192")
                .output()
                .expect("Expect creating disk image to succeed");
            std::process::Command::new("mcopy")
                .arg("-o")
                .args(&["-i", cloudinit_file_path.as_str()])
                .args(&["-s", cloud_init_directory.to_str().unwrap(), "::"])
                .output()
                .expect("Expect copying files to disk image to succeed");
            cloudinit_file_path
        }
    }

    struct UbuntuCloudInit {}
    impl CloudInit for UbuntuCloudInit {
        fn prepare(&self, tmp_dir: &TempDir, network: &GuestNetworkConfig) -> String {
            let cloudinit_file_path =
                String::from(tmp_dir.path().join("cloudinit").to_str().unwrap());

            let cloud_init_directory = tmp_dir.path().join("cloud-init").join("ubuntu");

            fs::create_dir_all(&cloud_init_directory)
                .expect("Expect creating cloud-init directory to succeed");

            let source_file_dir = std::env::current_dir()
                .unwrap()
                .join("resources")
                .join("cloud-init")
                .join("ubuntu");

            vec!["meta-data", "user-data"].iter().for_each(|x| {
                fs::copy(source_file_dir.join(x), cloud_init_directory.join(x))
                    .expect("Expect copying cloud-init meta-data to succeed");
            });

            let mut network_config_string = String::new();

            fs::File::open(source_file_dir.join("network-config"))
                .unwrap()
                .read_to_string(&mut network_config_string)
                .expect("Expected reading network-config file in to succeed");

            network_config_string = network_config_string.replace("192.168.2.1", &network.host_ip);
            network_config_string = network_config_string.replace("192.168.2.2", &network.guest_ip);
            network_config_string =
                network_config_string.replace("12:34:56:78:90:ab", &network.guest_mac);

            fs::File::create(cloud_init_directory.join("network-config"))
                .unwrap()
                .write_all(&network_config_string.as_bytes())
                .expect("Expected writing out network-config to succeed");

            std::process::Command::new("mkdosfs")
                .args(&["-n", "cidata"])
                .args(&["-C", cloudinit_file_path.as_str()])
                .arg("8192")
                .output()
                .expect("Expect creating disk image to succeed");

            vec!["user-data", "meta-data", "network-config"]
                .iter()
                .for_each(|x| {
                    std::process::Command::new("mcopy")
                        .arg("-o")
                        .args(&["-i", cloudinit_file_path.as_str()])
                        .args(&["-s", cloud_init_directory.join(x).to_str().unwrap(), "::"])
                        .output()
                        .expect("Expect copying files to disk image to succeed");
                });

            cloudinit_file_path
        }
    }

    fn prepare_os_disk(tmp_dir: &TempDir, image_name: &str) -> String {
        let src_osdisk = dirs::home_dir()
            .expect("Expect getting home directory to succeed")
            .join("workloads")
            .join(image_name);
        let dest_osdisk = tmp_dir.path().join(image_name);
        fs::copy(&src_osdisk, &dest_osdisk).expect("Expect copying OS disk to succeed");

        dest_osdisk.to_str().unwrap().to_owned()
    }

    fn prepare_tap(net: &GuestNetworkConfig) {
        assert!(std::process::Command::new("bash")
            .args(&[
                "-c",
                &format!("sudo ip tuntap add name {} mode tap", net.tap_name),
            ])
            .status()
            .expect("Expected creating interface to work")
            .success());

        assert!(std::process::Command::new("bash")
            .args(&[
                "-c",
                &format!("sudo ip addr add {}/24 dev {}", net.host_ip, net.tap_name),
            ])
            .status()
            .expect("Expected programming interface to work")
            .success());

        assert!(std::process::Command::new("bash")
            .args(&["-c", &format!("sudo ip link set dev {} up", net.tap_name)])
            .status()
            .expect("Expected upping interface to work")
            .success());
    }

    fn cleanup_tap(net: &GuestNetworkConfig) {
        assert!(std::process::Command::new("bash")
            .args(&[
                "-c",
                &format!("sudo ip tuntap de name {} mode tap", net.tap_name),
            ])
            .status()
            .expect("Expected deleting interface to work")
            .success());
    }

    #[derive(Debug)]
    enum SSHCommandError {
        Connection(std::io::Error),
        Handshake(ssh2::Error),
        Authentication(ssh2::Error),
        ChannelSession(ssh2::Error),
        Command(ssh2::Error),
    }

    fn ssh_command(ip: &str, command: &str) -> Result<String, SSHCommandError> {
        const DEFAULT_SSH_RETRIES: u8 = 6;
        const DEFAULT_SSH_TIMEOUT: u8 = 10;

        let retries = DEFAULT_SSH_RETRIES;
        let timeout = DEFAULT_SSH_TIMEOUT;
        let mut s = String::new();

        let mut counter = 0;
        loop {
            match (|| -> Result<(), SSHCommandError> {
                let tcp = TcpStream::connect(format!("{}:22", ip))
                    .map_err(SSHCommandError::Connection)?;
                let mut sess = ssh2::Session::new().unwrap();
                sess.set_tcp_stream(tcp);
                sess.handshake().map_err(SSHCommandError::Handshake)?;

                sess.userauth_password("cloud", "cloud123")
                    .map_err(SSHCommandError::Authentication)?;
                assert!(sess.authenticated());

                let mut channel = sess
                    .channel_session()
                    .map_err(SSHCommandError::ChannelSession)?;
                channel.exec(command).map_err(SSHCommandError::Command)?;

                // Intentionally ignore these results here as their failure
                // does not precipitate a repeat
                let _ = channel.read_to_string(&mut s);
                let _ = channel.close();
                let _ = channel.wait_close();
                Ok(())
            })() {
                Ok(_) => break,
                Err(e) => {
                    counter += 1;
                    if counter >= retries {
                        return Err(e);
                    }
                }
            };
            thread::sleep(std::time::Duration::new((timeout * counter).into(), 0));
        }
        Ok(s)
    }

    fn spawn_ch(os: &str, ci: &str, net: &GuestNetworkConfig) -> Child {
        let mut c = Command::new("./cloud-hypervisor");
        c.args(&[
            "--console",
            "off",
            "--serial",
            "tty",
            "--kernel",
            "target/target/release/hypervisor-fw",
            "--disk",
            &format!("path={}", os),
            &format!("path={}", ci),
            "--net",
            &format!("tap={},mac={}", net.tap_name, net.guest_mac),
        ]);

        eprintln!("Spawning: {:?}", c);
        c.spawn()
            .expect("Expect launching Cloud Hypervisor to succeed")
    }

    fn spawn_qemu(os: &str, ci: &str, net: &GuestNetworkConfig) -> Child {
        let mut c = Command::new("qemu-system-x86_64");
        c.args(&[
            "-machine",
            "q35,accel=kvm",
            "-cpu",
            "host,-vmx",
            "-kernel",
            "target/target/release/hypervisor-fw",
            "-display",
            "none",
            "-nodefaults",
            "-serial",
            "stdio",
            "-drive",
            &format!("id=os,file={},if=none", os),
            "-device",
            "virtio-blk-pci,drive=os,disable-legacy=on",
            "-drive",
            &format!("id=ci,file={},if=none,format=raw", ci),
            "-device",
            "virtio-blk-pci,drive=ci,disable-legacy=on",
            "-m",
            "1G",
            "-netdev",
            &format!(
                "tap,id=net0,ifname={},script=no,downscript=no",
                net.tap_name
            ),
            "-device",
            &format!("virtio-net-pci,netdev=net0,mac={}", net.guest_mac),
        ]);

        eprintln!("Spawning: {:?}", c);
        c.spawn().expect("Expect launching QEMU to succeed")
    }

    type HypervisorSpawn = fn(os: &str, ci: &str, net: &GuestNetworkConfig) -> Child;

    fn test_boot(image_name: &str, cloud_init: &dyn CloudInit, spawn: HypervisorSpawn) {
        let tmp_dir = TempDir::new("rhfw").expect("Expect creating temporary directory to succeed");
        let net = GuestNetworkConfig::new(COUNTER.fetch_add(1, Ordering::SeqCst) as u8);
        let ci = cloud_init.prepare(&tmp_dir, &net);
        let os = prepare_os_disk(&tmp_dir, image_name);

        prepare_tap(&net);

        let mut child = spawn(&os, &ci, &net);

        thread::sleep(std::time::Duration::from_secs(20));
        ssh_command(&net.guest_ip, "sudo shutdown -h now").expect("Expect SSH command to work");

        child.kill().unwrap();
        child.wait().unwrap();

        cleanup_tap(&net);
    }

    const BIONIC_IMAGE_NAME: &str = "bionic-server-cloudimg-amd64-raw.img";
    const FOCAL_IMAGE_NAME: &str = "focal-server-cloudimg-amd64-raw.img";
    const CLEAR_IMAGE_NAME: &str = "clear-31311-cloudguest.img";

    #[test]
    fn test_boot_qemu_bionic() {
        test_boot(BIONIC_IMAGE_NAME, &UbuntuCloudInit {}, spawn_qemu)
    }

    // Does not currently work:
    // #[test]
    fn test_boot_qemu_focal() {
        test_boot(FOCAL_IMAGE_NAME, &UbuntuCloudInit {}, spawn_qemu)
    }

    #[test]
    fn test_boot_qemu_clear() {
        test_boot(CLEAR_IMAGE_NAME, &ClearCloudInit {}, spawn_qemu)
    }

    #[test]
    fn test_boot_ch_bionic() {
        test_boot(BIONIC_IMAGE_NAME, &UbuntuCloudInit {}, spawn_ch)
    }

    #[test]
    fn test_boot_ch_focal() {
        test_boot(FOCAL_IMAGE_NAME, &UbuntuCloudInit {}, spawn_ch)
    }

    #[test]
    fn test_boot_ch_clear() {
        test_boot(CLEAR_IMAGE_NAME, &ClearCloudInit {}, spawn_ch)
    }
}