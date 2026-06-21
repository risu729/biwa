FROM lscr.io/linuxserver/openssh-server@sha256:835c1f321ff16b10e1e3d82ec3808a5253f02f73a98d7dca5b0786184f44e69a

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
