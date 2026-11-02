FROM lscr.io/linuxserver/openssh-server@sha256:0b46c755aac07057908077f2c7ff14f0ad923970bb6a3a67bcd008e72fa60050

# `PerSourcePenalties no` keeps the deliberate authentication failures in the suite from
# throttling the test runner's later connections.
RUN printf '\nAcceptEnv BIWA_TEST_* CDPATH\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
