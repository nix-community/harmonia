#!/usr/bin/env nix-shell
#!nix-shell -i bash -p nodejs nodePackages.npm
set -euo pipefail

# This script builds the Tailwind CSS for the harmonia landing page

cd "$(dirname "$0")"

# Check if node_modules exists
if [ ! -d "node_modules" ]; then
    echo "Installing dependencies..."
    npm install
fi

echo "Building Tailwind CSS..."
npm run build-css

echo "CSS built successfully at src/styles/output.css"