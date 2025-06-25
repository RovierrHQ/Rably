FROM rust:1.87-slim-bookworm

# Install basic dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev

# Set working directory
WORKDIR /app

# Copy everything
COPY . .

# Build the app
RUN cargo build --release

# Expose port
EXPOSE 8080

# Run the app
CMD ["./target/release/rably"]
