# The HTTP surface.
#
# TRAP: `require "faraday/retry"` is either a file inside the faraday gem or the
# separate faraday-retry gem. The Gemfile declares `faraday`, so that is what this
# resolves to — and neither a missing faraday-retry nor an unused faraday is right.
#
# PLANTED: nokogiri is required here and declared nowhere.

require "json"
require "faraday"
require "faraday/retry"
require "nokogiri"

require_relative "order"

module Shop
  class Client
    def initialize(url)
      @conn = Faraday.new(url: url)
    end

    def fetch(id)
      body = JSON.parse(@conn.get("/orders/#{id}").body)
      Order.new(body["id"], body["total_cents"])
    end

    def scrape(html)
      Nokogiri::HTML(html).css("h1").text
    end
  end
end
