FROM lscr.io/linuxserver/openssh-server@sha256:b07414bb329b86a00525c9256711a7855fc3a369e9f62c36f1de8d1923197580

# The suite deliberately fails authentication (unauthorized keys, wrong passwords), and
# sshd's per-source penalties would throttle the test runner's later connections.
# The image copies this file to /config/sshd/sshd_config on startup.
RUN printf '\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config
