-- ************************************************************ 
-- Title:	PUBLIC_DVD_DUDETAILSUMMARY_201609.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS DUDETAILSUMMARY owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.26	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_DUDETAILSUMMARY_201609010000.csv  
APPEND INTO TABLE DUDETAILSUMMARY	
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
DUID CHAR(10), 
START_DATE DATE "yyyy/mm/dd hh24:mi:ss", 
END_DATE DATE "yyyy/mm/dd hh24:mi:ss", 
DISPATCHTYPE CHAR(10), 
CONNECTIONPOINTID CHAR(10), 
REGIONID CHAR(10), 
STATIONID CHAR(10), 
PARTICIPANTID CHAR(10), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
TRANSMISSIONLOSSFACTOR FLOAT EXTERNAL, 
STARTTYPE CHAR(20), 
DISTRIBUTIONLOSSFACTOR FLOAT EXTERNAL, 
MINIMUM_ENERGY_PRICE FLOAT EXTERNAL, 
MAXIMUM_ENERGY_PRICE FLOAT EXTERNAL, 
SCHEDULE_TYPE CHAR(20), 
MIN_RAMP_RATE_UP FLOAT EXTERNAL, 
MIN_RAMP_RATE_DOWN FLOAT EXTERNAL, 
MAX_RAMP_RATE_UP FLOAT EXTERNAL, 
MAX_RAMP_RATE_DOWN FLOAT EXTERNAL, 
IS_AGGREGATED FLOAT EXTERNAL) 
-- End of Script 
