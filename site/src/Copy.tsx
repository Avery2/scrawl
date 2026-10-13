import { useState } from "react"

export function Copy({ text }: { text: string }) {
  const [done, setDone] = useState(false)
  return (
    <button
      className="copy"
      onClick={() => {
        navigator.clipboard?.writeText(text)
        setDone(true)
        setTimeout(() => setDone(false), 1400)
      }}
    >
      {done ? "Copied" : "Copy"}
    </button>
  )
}
