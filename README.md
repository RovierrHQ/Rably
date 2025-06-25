# Rably
An Open-source realtime micro-service that provides DX similar to Ably

## build docker ( about 1.2 GB)
```bash
docker build --platform linux/amd64 -t rably .
```
## slim build (about 12 MB)
for more info about slim[https://github.com/slimtoolkit/slim?tab=readme-ov-file#installation]
```bash
slim build --target rably:latest --tag rably:slim --image-build-arch amd64
```


## push to github
```bash
docker tag rably:slim ghcr.io/rovierrhq/rably:slim
docker push ghcr.io/rovierrhq/rably:slim
```

## run docker
```bash
docker run -d --name rably -p 8080:8080 rably:slim
```