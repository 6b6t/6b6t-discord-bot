FROM node:23

WORKDIR /app

COPY package*.json ./
COPY tsconfig.json ./
COPY src/ ./src/

RUN npm install
RUN npm run build

CMD ["node", "dist/index.js"]

#FROM node:20-alpine

#WORKDIR /app

# Copy package files first to leverage Docker cache
#COPY package*.json ./
#COPY tsconfig.json ./

# Install dependencies
#RUN npm install

# Copy source code
#COPY src/ ./src/

# Build TypeScript
#RUN npm run build

# Start the bot
#CMD ["node", "dist/index.js"]
