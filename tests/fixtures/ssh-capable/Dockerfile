FROM lscr.io/linuxserver/openssh-server@sha256:deec9e402eb9a656de1749c3ea93e99478a565cf187ecfb051f8446d93b12483

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
