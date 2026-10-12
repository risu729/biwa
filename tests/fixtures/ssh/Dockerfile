FROM lscr.io/linuxserver/openssh-server@sha256:0f535b0faa7448511b23528165e9fbd4f1fe563b861b3dcaa5ddb0ddbfe2254c

# The suite deliberately fails authentication (unauthorized keys, wrong passwords), and
# sshd's per-source penalties would throttle the test runner's later connections.
# The image copies this file to /config/sshd/sshd_config on startup.
RUN printf '\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config
