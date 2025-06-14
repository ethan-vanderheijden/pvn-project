import eventlet
from eventlet import wsgi
from oslo_config import cfg

from pvn_controller import create_app
from pvn_controller.config import load_config

load_config()
app = create_app()
wsgi.server(eventlet.listen((cfg.CONF.api.host_ip, cfg.CONF.api.port)), app)
