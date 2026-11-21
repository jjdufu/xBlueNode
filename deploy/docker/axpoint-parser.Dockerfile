FROM node:24-bookworm-slim AS node24

FROM python:3.11-slim-bookworm AS base
ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    HOME=/home/axp \
    AXPOINT_OCR_ENABLED=false
COPY --from=node24 /usr/local/bin/node /usr/local/bin/node
COPY --from=node24 /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -sf ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -sf ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
 && apt-get update && apt-get install -y --no-install-recommends \
      libpango-1.0-0 libpangocairo-1.0-0 libpangoft2-1.0-0 libcairo2 \
      libharfbuzz-subset0 libgdk-pixbuf-2.0-0 shared-mime-info \
      fonts-noto-cjk libgomp1 libxml2 libxslt1.1 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --create-home axp \
 && mkdir -p /var/lib/axpoint/models /var/lib/axpoint/data /var/lib/axpoint/exports \
 && chown -R axp:axp /var/lib/axpoint
WORKDIR /app
COPY axpoint-parser/pyproject.toml ./
COPY axpoint-parser/ ./
RUN if [ -f /app/charts/package.json ]; then \
      npm --prefix /app/charts ci --omit=dev && npm cache clean --force; \
    fi

FROM base AS lite
RUN pip install --no-cache-dir .
USER axp
RUN python -c "import importlib.util as u; \
    d = __import__('duckdb').connect() if u.find_spec('duckdb') else None; \
    [d.execute(f'INSTALL {e}') for e in ('ducklake','postgres','parquet')] if d else None"
EXPOSE 8001
HEALTHCHECK --interval=10s --timeout=3s --retries=6 CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8001/health').status==200 else 1)"
CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8001", "--workers", "2"]

FROM lite AS full
USER root
ENV AXPOINT_OCR_ENABLED=true
RUN apt-get update && apt-get install -y --no-install-recommends \
      libreoffice-core libreoffice-writer libreoffice-calc libreoffice-impress \
      freetds-dev libsasl2-dev libkrb5-dev unixodbc \
 && rm -rf /var/lib/apt/lists/* \
 && if python -c "import tomllib,sys; d=tomllib.load(open('pyproject.toml','rb')); sys.exit(0 if 'full' in d.get('project',{}).get('optional-dependencies',{}) else 1)"; then \
      pip install --no-cache-dir ".[full]"; \
    fi
USER axp
