#!/usr/bin/env bash
# Debian 12 一键安装 / 升级 Chronicle GM 联机服务端。
# 用法：sudo ./install.sh ./chronicle-server [域名]
#   第一个参数是编译好的 Linux 版 chronicle-server；第二个参数（可选）是域名，给出时写好 Nginx 配置。
set -euo pipefail
BIN="${1:?用法：sudo ./install.sh ./chronicle-server [域名]}"
DOMAIN="${2:-}"
HERE="$(cd "$(dirname "$0")" && pwd)"

id chronicle >/dev/null 2>&1 || useradd --system --home /var/lib/chronicle --shell /usr/sbin/nologin chronicle
install -d -o chronicle -g chronicle -m 750 /var/lib/chronicle
install -d -m 755 /opt/chronicle
install -d -m 750 /etc/chronicle
install -m 755 "$BIN" /opt/chronicle/chronicle-server.new
mv /opt/chronicle/chronicle-server.new /opt/chronicle/chronicle-server

if [ ! -f /etc/chronicle/server.env ]; then
  TOKEN="$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  cat > /etc/chronicle/server.env <<ENV
# 建房用的管理员令牌（POST /rooms 时放在 Authorization: Bearer 后面）。不要发给玩家。
CHRONICLE_ADMIN_TOKEN=$TOKEN
CHRONICLE_MAX_PLAYERS=4
CHRONICLE_MAX_ROOMS=20
CHRONICLE_MEMORY_LIMIT_MB=600
ENV
  chmod 640 /etc/chronicle/server.env
  chgrp chronicle /etc/chronicle/server.env
  echo "已生成管理员令牌：/etc/chronicle/server.env"
fi

install -m 644 "$HERE/chronicle-server.service" /etc/systemd/system/chronicle-server.service
systemctl daemon-reload
systemctl enable chronicle-server >/dev/null
systemctl restart chronicle-server

if [ -n "$DOMAIN" ]; then
  apt-get install -y nginx >/dev/null
  sed "s/chronicle.example.com/$DOMAIN/g" "$HERE/nginx-chronicle.conf" > /etc/nginx/sites-available/chronicle
  ln -sf /etc/nginx/sites-available/chronicle /etc/nginx/sites-enabled/chronicle
  if [ ! -f "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" ]; then
    echo "还没有证书：先 apt-get install -y certbot python3-certbot-nginx && certbot certonly --webroot -w /var/www/html -d $DOMAIN"
  else
    nginx -t && systemctl reload nginx
  fi
fi

sleep 1
curl -fsS http://127.0.0.1:7788/health && echo && echo "安装完成。"
