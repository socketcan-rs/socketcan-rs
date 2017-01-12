# -*- mode: ruby -*-
# vi: set ft=ruby :

# Vagrantfile API/syntax version. Don't touch unless you know what you're doing!
VAGRANTFILE_API_VERSION = "2"

$script = <<SCRIPT
apt-get update
apt-get install --yes graphviz curl doxygen build-essential can-utils git python3 python3-click inotify-tools
curl -s https://sh.rustup.rs > /rustup.sh
sudo -u vagrant -- sh /rustup.sh -y
mkdir -p /opt
git clone https://github.com/mbr/binbin /opt/binbin
ln -sf /opt/binbin/bin/rerun /usr/local/bin/rerun
ln -sf /opt/binbin/bin/repl /usr/local/bin/repl
SCRIPT

Vagrant.configure(VAGRANTFILE_API_VERSION) do |config|
  config.vm.box = "debian/jessie64"
  config.vm.provision "shell", inline: $script
  config.vm.synced_folder '.', '/vagrant', :disabled => true
end
