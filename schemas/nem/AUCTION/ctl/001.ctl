-- ************************************************************
-- Title:	PUBLIC_DVD_AUCTION_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS AUCTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_AUCTION_202401010000.CSV
APPEND INTO TABLE AUCTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
AUCTIONID CHAR(30),
AUCTIONDATE DATE "yyyy/mm/dd hh24:mi:ss",
NOTIFYDATE DATE "yyyy/mm/dd hh24:mi:ss",
STARTDATE DATE "yyyy/mm/dd hh24:mi:ss",
ENDDATE DATE "yyyy/mm/dd hh24:mi:ss",
DESCRIPTION CHAR(100),
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss",
AUTHORISEDBY CHAR(30),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
