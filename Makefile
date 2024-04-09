install_prometheus:
	pushd /tmp
	wget https://github.com/prometheus/prometheus/releases/download/v2.51.1/prometheus-2.51.1.linux-arm64.tar.gz
	popd
	sudo mkdir -p /opt/goodair/prometheus
	pushd /opt/goodair/prometheus
	sudo tar xf /tmp/prometheus-2.51.1.linux-arm64.tar.gz
	popd
	sudo sudo useradd --system prometheus
	sudo chown prometheus:prometheus /opt/goodair/prometheus

update_config:
	sudo cp prometheus.yml /opt/goodair/prometheus.yml
	sudo systemctl restart prometheus

update_service:
	sudo cp prometheus.service /etc/systemd/system/
	systemctl daemon-reload
	sudo systemctl restart prometheus


