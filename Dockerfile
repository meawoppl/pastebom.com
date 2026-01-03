# =============================================================================
# Stage 1: Build environment with KiCad
# =============================================================================
FROM kicad/kicad:8.0 AS base

# Install Python dependencies
RUN apt-get update && apt-get install -y \
    python3-pip \
    python3-venv \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create venv to avoid system package conflicts
RUN python3 -m venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

# Install Python packages
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# =============================================================================
# Stage 2: Runtime
# =============================================================================
FROM kicad/kicad:8.0

# Install curl for healthcheck
RUN apt-get update && apt-get install -y curl && rm -rf /var/lib/apt/lists/*

# Copy venv from builder
COPY --from=base /opt/venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

# Set up KiCad Python path
ENV PYTHONPATH="/usr/lib/python3/dist-packages:${PYTHONPATH}"

# Create app directory
WORKDIR /app

# Copy InteractiveHtmlBom (should be vendored or submodule)
COPY InteractiveHtmlBom/ /app/InteractiveHtmlBom/

# Copy application
COPY app/ /app/app/

# Create data directory
RUN mkdir -p /data/boms

# Environment
ENV STORAGE_PATH=/data/boms
ENV LOG_LEVEL=info

EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Run with uvicorn
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8080"]
