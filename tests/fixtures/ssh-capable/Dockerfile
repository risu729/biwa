FROM lscr.io/linuxserver/openssh-server@sha256:c8431b8982e7eb3935ebf9c46607d19c6417e39c12ce884cc66945df2e9334e8

# `PerSourcePenalties no` keeps the deliberate authentication failures in the suite from
# throttling the test runner's later connections.
RUN printf '\nAcceptEnv BIWA_TEST_* CDPATH\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
