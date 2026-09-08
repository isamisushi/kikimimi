FROM node:22-bookworm-slim AS build
WORKDIR /site
COPY package*.json ./
RUN npm ci
COPY . .
RUN npm test && npm run build

FROM nginxinc/nginx-unprivileged:stable-alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /site/dist /usr/share/nginx/html/kikimimi
EXPOSE 8080
