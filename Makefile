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

update_prometheus_config:
	sudo cp prometheus.yml /opt/goodair/prometheus.yml
	sudo systemctl restart prometheus

update_prometheus_service:
	sudo cp prometheus.service /etc/systemd/system/
	systemctl daemon-reload
	sudo systemctl restart prometheus

update_goodair_service:
	sudo cp goodair.service /etc/systemd/system/
	systemctl daemon-reload
	sudo systemctl restart goodair

install_goodair:
	cargo build --release
	sudo mkdir -p /opt/goodair/bin
	sudo cp -r target/release/* /opt/goodair/bin/

status:
	sudo systemctl status prometheus
	sudo systemctl status goodair
start:
	sudo systemctl start prometheus
	sudo systemctl start goodair
