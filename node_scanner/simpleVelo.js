const { request, gql } = require('graphql-request');
const path = require('path');
const fs = require('fs');
require('dotenv').config({ path: path.resolve(__dirname, '../.env') }); // Load .env from parent dir
const API_KEY = process.env.GRAPH_API_KEY;

const endpoint = 'https://gateway.thegraph.com/api/subgraphs/id/BsBDqDf6rJJyxKACZrCHAa8Gaf384cmL2hxfLaDuB8XM';

const query = `{
  factories(first: 5) {
    id
    poolCount
    txCount
    totalVolumeUSD
  }
  bundles(first: 5) {
    id
    ethPriceUSD
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
