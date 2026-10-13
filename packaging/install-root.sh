#!/usr/bin/env bash
# Holodrive, root side (only needed for the Samba panel). Run once with
# sudo from the checkout, after ./packaging/install.sh (which builds and
# stages the helper):
#
#   - installs holodrive-sambactl root-owned at /usr/local/bin
#   - installs its polkit action (admin password by default)
#   - writes /etc/polkit-1/rules.d/50-holodrive.rules, which lets YOU (the
#     sudo user) run the helper without a password
#   - installs /etc/holodrive/samba-deny.list (kept if it already exists)
#
# Undo: sudo ./packaging/install-root.sh --uninstall
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $EUID -eq 0 ]] || { echo "Run with sudo." >&2; exit 1; }
[[ -n "${SUDO_UID:-}" && "${SUDO_UID}" != 0 ]] || { echo "Run with sudo from your own account (SUDO_UID says who may use the helper)." >&2; exit 1; }

RULE=/etc/polkit-1/rules.d/50-holodrive.rules
if [[ "${1:-}" == "--uninstall" ]]; then
  rm -f /usr/local/bin/holodrive-sambactl \
        /usr/share/polkit-1/actions/tech.cybercore.holodrive.sambactl.policy "$RULE"
  echo "Removed the Holodrive helper and polkit files (deny list and smb.conf backups in /var/lib/holodrive kept)."
  exit 0
fi

user="$(getent passwd "$SUDO_UID" | cut -d: -f1)"
user_home="$(getent passwd "$SUDO_UID" | cut -d: -f6)"
helper="$user_home/.local/share/holodrive/holodrive-sambactl"
[[ -f "$helper" && ! -L "$helper" ]] || { echo "Run ./packaging/install.sh as your user first (it builds and stages the helper)." >&2; exit 1; }
[[ "$user" =~ ^[a-z_][a-z0-9_-]*$ ]] || { echo "unexpected user name $user" >&2; exit 1; }

install -o root -g root -m 0755 "$helper" /usr/local/bin/holodrive-sambactl
install -o root -g root -m 0644 packaging/tech.cybercore.holodrive.sambactl.policy \
  /usr/share/polkit-1/actions/tech.cybercore.holodrive.sambactl.policy
install -d -o root -g root -m 0755 /etc/holodrive /var/lib/holodrive
install -d -o root -g root -m 0700 /var/lib/holodrive/smb-backups
[[ -f /etc/holodrive/samba-deny.list ]] || install -o root -g root -m 0644 packaging/samba-deny.list /etc/holodrive/samba-deny.list
install -d -o root -g root -m 0755 /etc/polkit-1/rules.d
cat > "$RULE.tmp" <<RULES
// Written by Holodrive's install-root.sh: lets $user run Holodrive's Samba
// helper without a password (Holodrive runs as a service and can't ask).
polkit.addRule(function(action, subject) {
    if (action.id == "tech.cybercore.holodrive.sambactl" && subject.user == "$user") {
        return polkit.Result.YES;
    }
});
RULES
chmod 0644 "$RULE.tmp"
mv "$RULE.tmp" "$RULE"
echo
echo "Done: /usr/local/bin/holodrive-sambactl, polkit rule $RULE, deny list /etc/holodrive/samba-deny.list."
echo "Changes are logged: journalctl -t holodrive-sambactl"
