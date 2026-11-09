# Coach service: stdlib-only Python, no pip install, no build toolchain.
FROM python:3.12-slim
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
WORKDIR /app
COPY coach/ /app/coach/
USER 10001:10001
EXPOSE 18790
HEALTHCHECK --interval=15s --timeout=3s CMD python -c "import urllib.request as u; u.urlopen('http://127.0.0.1:18790/health', timeout=2)"
CMD ["python", "-m", "coach"]
