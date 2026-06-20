FROM lscr.io/linuxserver/openssh-server@sha256:d7ba00e0f66af202beebfa5cd96bad6698aa1543d5196e4d85dba340ea40d092

RUN printf '\nAcceptEnv BIWA_TEST_*\n' >> /etc/ssh/sshd_config \
	&& sed -i 's|^Subsystem[[:space:]]\+sftp[[:space:]].*$|Subsystem sftp /usr/lib/ssh/sftp-server -u 022|' /etc/ssh/sshd_config
