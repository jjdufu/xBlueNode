FROM node:24-bookworm-slim AS node24

FROM python:3.11-slim-bookworm AS base
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 HOME=/home/axp
RUN apt-get update && apt-get install -y --no-install-recommends \
      libpango-1.0-0 libpangoft2-1.0-0 libharfbuzz-subset0 libgdk-pixbuf-2.0-0 shared-mime-info fonts-noto-cjk \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --create-home axp
WORKDIR /app
COPY axpoint-parser/pyproject.toml ./
COPY axpoint-parser/ ./

FROM base AS lite
RUN pip install --no-cache-dir .
USER axp
RUN python -c "import importlib.util as u; \
    d = __import__('duckdb').connect() if u.find_spec('duckdb') else None; \
    [d.execute(f'INSTALL {e}') for e in ('ducklake','postgres')] if d else None"
EXPOSE 8001
HEALTHCHECK --interval=10s --timeout=3s --retries=6 CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8001/health').status==200 else 1)"
CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8001", "--workers", "2"]

FROM lite AS full
USER root
COPY --from=node24 /usr/local/ /usr/local/
RUN apt-get update && apt-get install -y --no-install-recommends \
      libreoffice-core libreoffice-writer libreoffice-calc libreoffice-impress \
 && rm -rf /var/lib/apt/lists/* \
 && if python -c "import tomllib,sys; d=tomllib.load(open('pyproject.toml','rb')); sys.exit(0 if 'full' in d.get('project',{}).get('optional-dependencies',{}) else 1)"; then \
      pip install --no-cache-dir ".[full]"; \
    fi \
 && if [ -f /app/charts/package.json ]; then npm --prefix /app/charts ci --omit=dev; fi
USER axp
