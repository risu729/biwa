FROM lscr.io/linuxserver/openssh-server@sha256:39ba37d50fdd6be1bf70644c871e5dcb9234ee79ac56424ea03ca08cadf1e7b0

# `PerSourcePenalties no` keeps the deliberate authentication failures in the suite from
# throttling the test runner's later connections.
RUN printf '\nAcceptEnv BIWA_TEST_* CDPATH\nPerSourcePenalties no\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
