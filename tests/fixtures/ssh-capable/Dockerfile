FROM lscr.io/linuxserver/openssh-server@sha256:5b8550a3b703eb4e5efb14a1f491370b7f765febfac5b0b2ed0321cdc74b1476

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
