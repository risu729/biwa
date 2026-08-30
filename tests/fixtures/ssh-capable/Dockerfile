FROM lscr.io/linuxserver/openssh-server@sha256:ec7af01dd539b201b56d6c3f0a4c2c9ca2a7736512920bc5e6e1d22989265b52

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
