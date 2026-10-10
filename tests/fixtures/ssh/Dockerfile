FROM lscr.io/linuxserver/openssh-server@sha256:46f115de7c251558297e7e87566fc3fc08544b63e55502b5cf294454db5d29d1

# The suite deliberately fails authentication (unauthorized keys, wrong passwords), and
# sshd's per-source penalties would throttle the test runner's later connections.
# The image copies this file to /config/sshd/sshd_config on startup.
RUN printf '\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config
