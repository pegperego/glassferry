#!/bin/sh
# Local, unprivileged OpenSSH fixture for manual and integration tests.
#
#   scripts/test-sshd.sh start   # sshd on 127.0.0.1:2222 (shell) and :2223 (SFTP only)
#   scripts/test-sshd.sh stop
#
# Hosts (use .dev-sshd/ssh as the ssh binary, it adds -F .dev-sshd/ssh_config):
#   gf-local      key with passphrase "segreta", host key prompt on first use
#   gf-sftponly   ForceCommand internal-sftp (no shell)
#   gf-rc         RemoteCommand + RequestTTY force in the client config
#   gf-slow       like gf-local, through a relay adding 20 ms each way
#
# Example: cargo run --example spike -- "$PWD/.dev-sshd/ssh" gf-local segreta

set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
DIR="$ROOT/.dev-sshd"
SFTP_SERVER=/usr/libexec/sftp-server
[ -x "$SFTP_SERVER" ] || SFTP_SERVER=/usr/lib/openssh/sftp-server

stop() {
    for pidfile in "$DIR"/sshd-*.pid; do
        [ -f "$pidfile" ] || continue
        kill "$(cat "$pidfile")" 2>/dev/null || true
        rm -f "$pidfile"
    done
}

write_sshd_config() { # port name extra
    cat > "$DIR/sshd-$2.conf" <<EOF
Port $1
ListenAddress 127.0.0.1
HostKey $DIR/hostkey
PidFile $DIR/sshd-$2.pid
AuthorizedKeysFile $DIR/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
Subsystem sftp $SFTP_SERVER
$3
EOF
}

write_host() { # alias port extra
    cat >> "$DIR/ssh_config" <<EOF
Host $1
  HostName 127.0.0.1
  Port $2
  User $(id -un)
  IdentityFile $DIR/userkey
  IdentitiesOnly yes
  IdentityAgent none
  UserKnownHostsFile $DIR/known_hosts
  StrictHostKeyChecking ask
$3

EOF
}

start() {
    stop
    mkdir -p "$DIR"
    [ -f "$DIR/hostkey" ] || ssh-keygen -q -t ed25519 -N '' -f "$DIR/hostkey"
    [ -f "$DIR/userkey" ] || ssh-keygen -q -t ed25519 -N 'segreta' -f "$DIR/userkey"
    cp "$DIR/userkey.pub" "$DIR/authorized_keys"
    chmod 600 "$DIR/authorized_keys"
    rm -f "$DIR/known_hosts" "$DIR/ssh_config"

    write_sshd_config 2222 shell ""
    write_sshd_config 2223 sftponly "ForceCommand internal-sftp"
    write_host gf-local 2222 ""
    write_host gf-sftponly 2223 ""
    write_host gf-rc 2222 "  RemoteCommand echo hello
  RequestTTY force"
    write_host gf-slow 2222 "  ProxyCommand python3 $ROOT/scripts/delay-relay.py 127.0.0.1 2222 20"

    printf '#!/bin/sh\nexec /usr/bin/ssh -F "%s/ssh_config" "$@"\n' "$DIR" > "$DIR/ssh"
    chmod +x "$DIR/ssh"

    /usr/sbin/sshd -f "$DIR/sshd-shell.conf" -E "$DIR/sshd-shell.log"
    /usr/sbin/sshd -f "$DIR/sshd-sftponly.conf" -E "$DIR/sshd-sftponly.log"
    echo "sshd listening on 127.0.0.1:2222 and :2223 (fixture in $DIR)"
}

case "${1:-}" in
    start) start ;;
    stop) stop ;;
    *) echo "usage: $0 start|stop" >&2; exit 2 ;;
esac
