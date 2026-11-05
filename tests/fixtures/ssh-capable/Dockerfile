FROM lscr.io/linuxserver/openssh-server@sha256:aa4e1b3916e8555f24e574e6108b0b89a1b20362b0a11800a71280b45999649e

# `PerSourcePenalties no` keeps the deliberate authentication failures in the suite from
# throttling the test runner's later connections.
RUN printf '\nAcceptEnv BIWA_TEST_* CDPATH\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
