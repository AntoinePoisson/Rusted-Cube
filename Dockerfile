FROM rust:latest

## Get LocalFile
WORKDIR /app/minecraft
COPY . /app/minecraft

## Update System
RUN apt-get update && apt-get install -y

## Expose Port
EXPOSE 8080

RUN ls

## Get Wasm-Pack
RUN curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

## Install npm
RUN apt install nodejs -y
RUN apt install npm -y
RUN npm install -g n
RUN n latest

## Install http-server from NPM
RUN npm i http-server -g

## Install projet dependance
# RUN cargo install --path .

## Build projet
# RUN cargo build .

# RUN wasm-pack build --target web --out-name web

CMD [ "http-server", "-c-1", "-f", "/index.html",  "-p", "8080" ]