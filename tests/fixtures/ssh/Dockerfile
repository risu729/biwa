FROM lscr.io/linuxserver/openssh-server@sha256:2a48f9ce01f61c1d7b376b7be99bd12801a3ecd9f339a4c7e7698d529e8d0b47

# The suite deliberately fails authentication (unauthorized keys, wrong passwords), and
# sshd's per-source penalties would throttle the test runner's later connections.
# The image copies this file to /config/sshd/sshd_config on startup.
RUN printf '\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config
