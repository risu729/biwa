FROM lscr.io/linuxserver/openssh-server@sha256:0b46c755aac07057908077f2c7ff14f0ad923970bb6a3a67bcd008e72fa60050

# The suite deliberately fails authentication (unauthorized keys, wrong passwords), and
# sshd's per-source penalties would throttle the test runner's later connections.
# The image copies this file to /config/sshd/sshd_config on startup.
RUN printf '\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config
