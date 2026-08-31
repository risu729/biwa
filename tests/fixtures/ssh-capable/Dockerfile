FROM lscr.io/linuxserver/openssh-server@sha256:2a48f9ce01f61c1d7b376b7be99bd12801a3ecd9f339a4c7e7698d529e8d0b47

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
