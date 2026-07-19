FROM lscr.io/linuxserver/openssh-server@sha256:9c5e178975fcc3917853f5e37cbf135ad7deb11de504ab0f460cc81a2e1eb539

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
