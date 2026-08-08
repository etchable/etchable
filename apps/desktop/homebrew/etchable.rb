# Template for the etchable app cask. release-app.yml fills in the version and
# sha256 from the built .dmg and pushes the result to etchable/homebrew-etchable.
cask "etchable" do
  version "VERSION_PLACEHOLDER"
  sha256 "SHA256_PLACEHOLDER"

  url "https://github.com/etchable/etchable/releases/download/v#{version}/etchable_#{version}_aarch64.dmg"
  name "etchable"
  desc "etchable desktop app"
  homepage "https://github.com/etchable/etchable"

  depends_on macos: ">= :ventura"
  depends_on arch: :arm64

  app "etchable.app"

  zap trash: "~/Library/Application Support/net.etchable.app"
end
