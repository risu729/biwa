FROM lscr.io/linuxserver/openssh-server@sha256:aa4e1b3916e8555f24e574e6108b0b89a1b20362b0a11800a71280b45999649e

# The suite deliberately fails authentication (unauthorized keys, wrong passwords), and
# sshd's per-source penalties would throttle the test runner's later connections.
# The image copies this file to /config/sshd/sshd_config on startup.
RUN printf '\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config
