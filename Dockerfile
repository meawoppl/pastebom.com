# =============================================================================
# Stage 1: Build environment with KiCad
# =============================================================================
FROM kicad/kicad:9.0 AS base

USER root

# Install Python dependencies and git for cloning
RUN apt-get update && apt-get install -y \
    python3-pip \
    python3-venv \
    curl \
    git \
    && rm -rf /var/lib/apt/lists/*

# Clone InteractiveHtmlBom fork
ARG IBOM_REPO=https://github.com/openscopeproject/InteractiveHtmlBom.git
ARG IBOM_BRANCH=master
RUN git clone --depth 1 --branch ${IBOM_BRANCH} ${IBOM_REPO} /opt/InteractiveHtmlBom

# Create venv to avoid system package conflicts
RUN python3 -m venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

# Install Python packages
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# =============================================================================
# Stage 2: Runtime
# =============================================================================
FROM kicad/kicad:9.0

USER root

# Install curl for healthcheck
RUN apt-get update && apt-get install -y curl && rm -rf /var/lib/apt/lists/*

# Copy venv from builder
COPY --from=base /opt/venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

# Set up KiCad Python path - append after venv to avoid conflicts
ENV PYTHONPATH="/opt/venv/lib/python3.11/site-packages:/usr/lib/python3/dist-packages"

# Create app directory
WORKDIR /app

# Copy InteractiveHtmlBom from build stage
COPY --from=base /opt/InteractiveHtmlBom /app/InteractiveHtmlBom

# Copy application
COPY app/ /app/app/

# Environment
ENV LOG_LEVEL=info

EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Run with uvicorn
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8080"]
