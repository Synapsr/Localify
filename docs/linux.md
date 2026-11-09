# Localify on Linux

This guide covers Linux-specific setup details and
troubleshooting. For general usage, see the
[user guide](README.md).

## Install

```bash
curl -fsSL https://localify.dev/install.sh | sh
```

The script installs the x86_64 build of the latest release in `~/.local/bin`, after
checking its SHA-256, then runs `localify install`. Run it again to update. Other
architectures build from source. Over SSH, use `ssh -t` so `sudo` can ask for your
password, or have passwordless `sudo` (see [No Root After the Install](#no-root-after-the-install)).

## Supported Distributions

Localify is tested on **Ubuntu 22.04+** and **Debian 12+**.
It should work on any distribution that uses:

- **systemd-resolved** for DNS
- **update-ca-certificates** for the system trust store

Other distributions (Fedora, Arch, etc.) may work but
are not yet officially supported.

## How It Works on Linux

### DNS Resolution

On macOS, Localify uses `/etc/resolver/localify` to route `.localify`
DNS queries. On Linux, it uses **systemd-resolved** with
a drop-in configuration file:

```text
/etc/systemd/resolved.conf.d/localify.conf
```

This file tells systemd-resolved to forward all `.localify`
queries to Localify's built-in DNS server on port 1053.
Other DNS queries are unaffected.

The file is created automatically by `localify install`
and removed by `localify uninstall`.

### Certificate Trust

On macOS, the Root CA is trusted in your login keychain, for your account only, when
you run `localify trust`.
On Linux, Localify copies the Root CA to the system
certificate store and runs `update-ca-certificates`:

```text
/usr/local/share/ca-certificates/localify-ca.crt
```

This makes the CA trusted by `curl`, `wget`, `git`,
Electron apps, and non-sandboxed browsers.

## No Root After the Install

`localify install` asks for your `sudo` password once. It runs a script of system tools,
never the Localify binary, that writes the systemd units, the DNS drop-in and the Root CA.
After that nothing of Localify's runs as root:

- systemd holds ports 80 and 443 on `127.0.0.1` and `::1`, and hands them to the daemon.
- The daemon runs as you, from your `~/.config/localify/`. A static route can only serve
  files you can read.
- `stop`, `start`, `restart`, `register` and updates need no password.
- Only this machine can connect until you run `localify lan enable`.

Localify refuses to run under `sudo`. A taken port stops the install with the old setup
back in place: find what holds it with `sudo ss -ltnp`.

With no terminal, `localify install` only works with passwordless `sudo`.

### Run as Root, or With No systemd

As root, or in a container with no systemd, Localify works as a plain process: files in
`/etc/localify/`, and it listens on every interface unless you set `lan_access = false`.
A root daemon's static routes can serve any file on the machine, so point them only at
folders you would share with anyone who can reach the port.

## Snap Browsers and Certificate Trust

**This is the most common issue on Linux.**

Snap-packaged browsers (Firefox, Chromium) run in a
sandbox and **cannot access the system's certificate
trust store**. Even with proper system CA installation,
these browsers will show certificate warnings for
`.localify` domains.

This does **not** affect:

- `curl`, `wget`, `git`, and other CLI tools
- Non-snap browsers (installed via apt/deb)
- Electron apps (VS Code, Slack, etc.)

### Fix: Import the CA with certutil

This is a **one-time** step per browser. Once the Root CA
is imported, all `.localify` domains (including newly
registered ones) are automatically trusted.

**1. Install certutil:**

```bash
sudo apt install libnss3-tools
```

**2. Import the Localify CA into your browser:**

**Snap Firefox:**

```bash
certutil -A -n "Localify Local Development CA" -t "CT,C,C" \
  -i ~/.config/localify/data/ca.crt \
  -d sql:$(find ~/snap/firefox/common/.mozilla/firefox \
    -name '*.default*' -type d | head -1)/
```

**Snap Chromium:**

```bash
certutil -A -n "Localify Local Development CA" -t "CT,C,C" \
  -i ~/.config/localify/data/ca.crt \
  -d sql:$(find ~/snap/chromium -name 'nssdb' \
    -type d | head -1)/
```

No browser restart is needed after import.

### Why Does This Happen?

Snap applications are sandboxed using AppArmor and
seccomp. They cannot load the host system's p11-kit
trust modules, which is how most Linux applications
discover trusted CAs. The `certutil` command writes
directly into the browser's own NSS certificate
database, bypassing the sandbox limitation.

### Removing the CA

To remove the Localify CA from a snap browser:

**Firefox:**

```bash
certutil -D -n "Localify Local Development CA" \
  -d sql:$(find ~/snap/firefox/common/.mozilla/firefox \
    -name '*.default*' -type d | head -1)/
```

**Chromium:**

```bash
certutil -D -n "Localify Local Development CA" \
  -d sql:$(find ~/snap/chromium -name 'nssdb' \
    -type d | head -1)/
```

## Troubleshooting

### DNS Not Resolving

If `curl https://myapp.localify` fails with "Could not
resolve host":

**1. Check if systemd-resolved has the config:**

```bash
resolvectl status
```

Look for `DNS Servers: 127.0.0.1:1053` and
`DNS Domain: ~localify` in the global section.

**2. Check if the Localify DNS server responds:**

```bash
dig @127.0.0.1 -p 1053 myapp.localify
```

If this works but `resolvectl query` doesn't, the
drop-in config may need a restart:

```bash
sudo systemctl restart systemd-resolved
```

**3. Verify the drop-in file exists:**

```bash
cat /etc/systemd/resolved.conf.d/localify.conf
```

If missing, re-run `localify install`.

### Port Already in Use

On Linux, check which process is using a port with `ss`:

```bash
sudo ss -tlnp | grep ':80\b'
sudo ss -tlnp | grep ':443\b'
sudo ss -tlnp | grep ':1053\b'
```

Common culprits: Apache (`apache2`), nginx, or another
Localify instance. Stop the conflicting service or change
Localify's ports in `~/.config/localify/config.toml`, then run `localify install`.

### Auto-Start

`localify install` sets this up: systemd starts the daemon at boot, and `localify stop`
stays stopped across a reboot. See [No Root After the Install](#no-root-after-the-install).
