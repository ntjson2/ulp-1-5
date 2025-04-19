const { request, gql } = require('graphql-request');
const path = require('path');
const fs = require('fs');
require('dotenv').config({ path: path.resolve(__dirname, '../.env') }); // Load .env from parent dir
const API_KEY = process.env.GRAPH_API_KEY;

const endpoint = 'https://gateway.thegraph.com/api/subgraphs/id/3115xfkzXPrYzbqDHTiWGtzRDYNXBxs8dyitva6J18jf';

const query = `{
  assets(first: 5) {
    id
    key
    decimal
    adoptedDecimal
  }
  assetStatuses(first: 5) {
    id
    status
  }
}`;

const headers = {
  Authorization: `Bearer ${API_KEY}`,
};

async function fetchData() {
  try {
    const data = await request(endpoint, query, {}, headers);
    console.log(data);
  } catch (error) {
    console.error('Error fetching data:', error);
  }
}

fetchData();
