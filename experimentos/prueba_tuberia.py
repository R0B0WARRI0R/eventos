"""Mide cuándo llegan las líneas de un proceso hijo leído por tubería.

Si el hijo acumula la salida en un búfer, las líneas llegan todas juntas al final
en vez de una por una. Eso decide si un recolector por tubería ve los eventos a tiempo.
"""
import os
import subprocess
import sys
import time

HIJO = "import time\nfor i in range(3):\n    print('linea', i)\n    time.sleep(1)\n"


def medir(nombre, cmd, env=None):
    t0 = time.monotonic()
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
    llegadas = []
    for linea in p.stdout:
        llegadas.append((round(time.monotonic() - t0, 2), linea.decode(errors="replace").strip()))
    p.wait()
    print(f"{nombre}: {llegadas}")


base = dict(os.environ)
base.pop("PYTHONUNBUFFERED", None)
medir("python por tuberia", [sys.executable, "-c", HIJO], env=base)
medir("python -u por tuberia", [sys.executable, "-u", "-c", HIJO], env=base)
con_env = dict(base, PYTHONUNBUFFERED="1")
medir("python con PYTHONUNBUFFERED=1", [sys.executable, "-c", HIJO], env=con_env)
medir("ping.exe nativo", ["ping", "-n", "3", "127.0.0.1"])
