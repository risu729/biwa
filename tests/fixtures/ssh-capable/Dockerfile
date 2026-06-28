FROM lscr.io/linuxserver/openssh-server@sha256:67d4c3a1402179a6579aa217a38b52ced557eb8a0c17a8e32fe986a4549fdee4

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
