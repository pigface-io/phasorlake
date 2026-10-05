-- ************************************************************
-- Title:	PUBLIC_DVD_AUCTION_CALENDAR_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS AUCTION_CALENDAR owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_AUCTION_CALENDAR_202401010000.CSV
APPEND INTO TABLE AUCTION_CALENDAR
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONTRACTYEAR FLOAT EXTERNAL,
QUARTER FLOAT EXTERNAL,
STARTDATE DATE "yyyy/mm/dd hh24:mi:ss",
ENDDATE DATE "yyyy/mm/dd hh24:mi:ss",
NOTIFYDATE DATE "yyyy/mm/dd hh24:mi:ss",
PAYMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
RECONCILIATIONDATE DATE "yyyy/mm/dd hh24:mi:ss",
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
PRELIMPURCHASESTMTDATE DATE "yyyy/mm/dd hh24:mi:ss",
PRELIMPROCEEDSSTMTDATE DATE "yyyy/mm/dd hh24:mi:ss",
FINALPURCHASESTMTDATE DATE "yyyy/mm/dd hh24:mi:ss",
FINALPROCEEDSSTMTDATE DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
